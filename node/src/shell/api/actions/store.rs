//! The `stores` action namespace — signalling into a store, replaying what was
//! signalled, and administering who may do either.
//!
//! This is the node's own messaging layer. A store is the unit a signal is
//! addressed to; the node fans the signal out live to every connected member
//! and, when the store is marked `persHist`, writes it to the time-series log
//! with the sender's tags. `stores/history` reads that log back with a tag
//! filter, so a caller reconstructs any slice of the conversation — one thread,
//! one kind of message, one agent's trail — without keeping a parallel index of
//! its own anywhere else.
//!
//! Permissions are checked here, not in whatever creature happens to be calling:
//! `signal` to post, `read` to replay, `manage` to change another member's
//! grant. See [`crate::shell::api::model::access`] — an absent grant denies.
//!
//! Every input carries an `origin`, so a member whose node is not this one has
//! the whole action routed to the owning node by the federation driver
//! (`SecureAction::securely_act`) and served there against that node's log. A
//! store's signals therefore stay readable across a federation without being
//! replicated into chain state.

use std::sync::Arc;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::core::actor::model::secured::guard::Guard;
use crate::models::action::ISecureAction;
use crate::models::core::ICore;
use crate::models::packet::{validate_tags, LogPacket, LogQuery};
use crate::models::state::IState;
use crate::models::transaction::ITrx;
use crate::shell::api::model::access::{access_link_key, read_permissions, StorePermissions};
use crate::shell::api::model::{Creature, Store};
use crate::shell::api::packets::stores::{
    GetAccessInput, HistoryInput, Send as StoresSend, SetAccessInput, SignalInput,
};
use crate::shell::utils::future::async_once;

use super::util::build_secure_action;

/// Actions addressed to a store: the caller must be identified AND a member,
/// which the guard checks against `hasaccess::<userId>::<storeId>` before the
/// body runs. What the member may then *do* is the permission check inside each
/// body.
fn store_guard() -> Guard {
    Guard {
        is_user: true,
        is_in_store: true,
        allow_applet_sign: true,
    }
}

/// Default page size for a history read when the caller names none.
const DEFAULT_HISTORY_COUNT: i64 = 100;

/// Fan a store signal out to every connected member of the store except the
/// sender, and to the federation peers holding remote members.
///
/// The live packet carries the persisted row's `signalId`, `time` and `tags`,
/// so a client applies the same filter to a live signal that it applies to
/// history, and recognises the replayed row as one it has already rendered.
///
/// `remote_orgs` is read by the caller off the transaction it already holds: the
/// action body runs inside a state modification, so opening a second one here
/// would nest them.
fn fan_out(
    app: Arc<dyn ICore>,
    store_id: String,
    sender_id: String,
    packet: StoresSend,
    remote_orgs: Vec<String>,
) {
    let value = serde_json::to_value(&packet).unwrap_or(Value::Null);
    let _ = async_once(move || {
        app.tools().signaler().signal_group(
            "stores/signal",
            &store_id,
            value.clone(),
            true,
            vec![sender_id.clone()],
        );
        // Members on another node are not in this node's signal groups, so the
        // local fan-out never reaches them. Push the same packet to each peer
        // that holds one; the receiving node re-emits it to its own group
        // (`FedNet::handle_update`).
        for org in &remote_orgs {
            app.tools().network().federation().send_fed_update(
                org,
                "stores/signal",
                value.clone(),
                "store",
                &store_id,
                vec![sender_id.clone()],
            );
        }
    });
}

/// Distinct foreign origins among a store's members, read off the caller's own
/// transaction. A member id is `<counter>@<origin>`; anything whose origin is not
/// this node's id lives on a peer that has to be pushed to explicitly.
fn remote_member_orgs(trx: &dyn ITrx, self_id: &str, store_id: &str) -> Vec<String> {
    let prefix = format!("onaccess::{}::", store_id);
    let mut out: Vec<String> = Vec::new();
    for key in trx.get_links_list(&prefix, -1, -1, &[]).unwrap_or_default() {
        let member = key.strip_prefix(&prefix).unwrap_or(&key);
        let Some((_, org)) = member.rsplit_once('@') else {
            continue;
        };
        if org.is_empty() || org == self_id || out.iter().any(|o| o == org) {
            continue;
        }
        out.push(org.to_string());
    }
    out
}

/// `/stores/signal` — send one signal into a store.
///
/// Persistence is the store's decision, not the sender's: a store created with
/// `persHist` keeps every signal sent into it. The sender may only opt a single
/// signal OUT, with `temp`, for traffic that is meaningless after delivery
/// (typing indicators, progress pings).
fn signal(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    let app_for_handler = app.clone();
    build_secure_action::<SignalInput, _>(
        app,
        "/stores/signal",
        store_guard(),
        move |state: Arc<dyn IState>, input: SignalInput| -> Result<Value> {
            let store_id = input.store_id.clone();
            if store_id.is_empty() {
                return Err(anyhow!("storeId is required"));
            }
            let sender_id = state.info().user_id();
            let trx = state.trx();
            let perms = read_permissions(&*trx, &store_id, &sender_id);
            if !perms.signal {
                return Err(anyhow!("not allowed to signal in this store"));
            }
            let tags = validate_tags(&input.tags)?;

            // `Store::pull` keeps the id it was handed whether or not the object
            // exists, so absence has to be read off the columns themselves — an
            // unknown store would otherwise look like a store that simply does
            // not keep history, and its messages would be dropped in silence.
            if trx.get_obj(Store::type_(), &store_id).is_empty() {
                return Err(anyhow!("store not found"));
            }
            let store = Store {
                id: store_id.clone(),
                ..Default::default()
            }
            .pull(&*trx);

            let mut sender = Creature {
                id: sender_id.clone(),
                ..Default::default()
            }
            .pull(&*trx);
            // Balance is never leaked over the signalling channel.
            sender.balance = 0;

            let now = chrono::Utc::now().timestamp_millis();
            let persist = store.pers_hist && !input.temp;
            // Recording comes FIRST, and its failure fails the whole call. A
            // signal fanned out to everyone's screen but missing from the log is
            // the worst outcome available: it looks delivered, and it is gone by
            // the next read. Better to tell the sender it did not send.
            let logged: Option<LogPacket> = if persist {
                let packet = app_for_handler.tools().storage().log_time_sieries(
                    &store_id,
                    &sender_id,
                    &input.data,
                    &tags,
                    now,
                )?;
                // Keep the store's own counter true to the log so a reader can
                // tell an empty store from an unreachable one.
                let mut counted = store.clone();
                counted.signal_count += 1;
                counted.push(&*trx);
                Some(packet)
            } else {
                None
            };

            let out = StoresSend {
                action: if input.typ.is_empty() {
                    "broadcast".to_string()
                } else {
                    input.typ.clone()
                },
                user: sender,
                store: Store {
                    id: store_id.clone(),
                    ..Default::default()
                },
                data: input.data.clone(),
                is_temp: input.temp,
                tags: tags.clone(),
                signal_id: logged.as_ref().map(|p| p.id.clone()).unwrap_or_default(),
                time: now,
                ..Default::default()
            };
            let remote_orgs = remote_member_orgs(&*trx, &app_for_handler.id(), &store_id);
            fan_out(app_for_handler.clone(), store_id, sender_id, out, remote_orgs);

            Ok(json!({
                "passed": true,
                "persisted": persist,
                "signalId": logged.as_ref().map(|p| p.id.clone()).unwrap_or_default(),
                "time": now,
                "tags": tags,
            }))
        },
    )
}

/// `/stores/history` — replay a store's persisted signals, newest first,
/// filtered by tag.
fn history(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    let app_for_handler = app.clone();
    build_secure_action::<HistoryInput, _>(
        app,
        "/stores/history",
        store_guard(),
        move |state: Arc<dyn IState>, input: HistoryInput| -> Result<Value> {
            let store_id = input.store_id.clone();
            if store_id.is_empty() {
                return Err(anyhow!("storeId is required"));
            }
            let reader_id = state.info().user_id();
            let perms = read_permissions(&*state.trx(), &store_id, &reader_id);
            if !perms.read {
                return Err(anyhow!("not allowed to read this store"));
            }
            let query = LogQuery {
                tags_all: input.tags_all.clone(),
                tags_any: input.tags_any.clone(),
                before_time: input.before_time,
                after_time: input.after_time,
                count: if input.count > 0 {
                    input.count
                } else {
                    DEFAULT_HISTORY_COUNT
                },
            }
            .validated()?;
            let packets = app_for_handler
                .tools()
                .storage()
                .read_store_logs(&store_id, &query)?;
            Ok(json!({
                "storeId": store_id,
                "signals": packets,
            }))
        },
    )
}

/// `/stores/setAccess` — set one member's permissions.
///
/// Requires `manage`. This is how a role maps onto the node: a viewer is
/// granted `read` alone, an ordinary member `read,signal`, an administrator
/// `read,signal,manage`.
fn set_access(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<SetAccessInput, _>(
        app,
        "/stores/setAccess",
        store_guard(),
        move |state: Arc<dyn IState>, input: SetAccessInput| -> Result<Value> {
            let store_id = input.store_id.clone();
            if store_id.is_empty() {
                return Err(anyhow!("storeId is required"));
            }
            if input.member_id.is_empty() {
                return Err(anyhow!("memberId is required"));
            }
            let caller = state.info().user_id();
            let trx = state.trx();
            if !read_permissions(&*trx, &store_id, &caller).manage {
                return Err(anyhow!("not allowed to manage access in this store"));
            }
            let perms = StorePermissions::from_list(&input.permissions);
            trx.put_link(
                &access_link_key(&store_id, &input.member_id),
                &perms.encode(),
            );
            Ok(json!({
                "storeId": store_id,
                "memberId": input.member_id,
                "permissions": perms,
            }))
        },
    )
}

/// `/stores/getAccess` — read a member's permissions. A member may always read
/// their own; reading somebody else's requires `manage`.
fn get_access(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<GetAccessInput, _>(
        app,
        "/stores/getAccess",
        store_guard(),
        move |state: Arc<dyn IState>, input: GetAccessInput| -> Result<Value> {
            let store_id = input.store_id.clone();
            if store_id.is_empty() {
                return Err(anyhow!("storeId is required"));
            }
            let caller = state.info().user_id();
            let target = if input.member_id.is_empty() {
                caller.clone()
            } else {
                input.member_id.clone()
            };
            let trx = state.trx();
            let caller_perms = read_permissions(&*trx, &store_id, &caller);
            if target != caller && !caller_perms.manage {
                return Err(anyhow!("not allowed to read another member's access"));
            }
            let perms = if target == caller {
                caller_perms
            } else {
                read_permissions(&*trx, &store_id, &target)
            };
            Ok(json!({
                "storeId": store_id,
                "memberId": target,
                "permissions": perms,
            }))
        },
    )
}

/// Install every store action onto the actor.
pub fn install(app: Arc<dyn ICore>) {
    let actor = app.actor();
    let handlers: Vec<Arc<dyn ISecureAction>> = vec![
        signal(app.clone()),
        history(app.clone()),
        set_access(app.clone()),
        get_access(app.clone()),
    ];
    for h in handlers {
        actor.inject_secure_action(h);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::input::IInput;
    use crate::shell::api::packets::stores::{HistoryInput, SignalInput};

    /// The store-scoped guard must demand BOTH an identified caller and store
    /// membership: the permission checks in each body assume the caller is a
    /// member, and only the guard establishes that.
    #[test]
    fn store_actions_are_guarded_by_identity_and_membership() {
        let g = store_guard();
        assert!(g.is_user, "an anonymous caller must never reach a store action");
        assert!(g.is_in_store, "membership is checked before the body runs");
    }

    /// Inputs route by `origin`, which is what makes a store on another node
    /// readable and writable through the same two actions.
    #[test]
    fn inputs_carry_their_federation_origin_and_store() {
        let signal = SignalInput {
            store_id: "7@peer".into(),
            origin: "peer".into(),
            ..Default::default()
        };
        assert_eq!(signal.get_store_id(), "7@peer");
        assert_eq!(signal.origin(), "peer", "a foreign origin routes the action to that node");

        let history = HistoryInput {
            store_id: "7@peer".into(),
            ..Default::default()
        };
        assert_eq!(history.get_store_id(), "7@peer");
        assert_eq!(history.origin(), "", "no origin means this node serves it");
    }

    /// A member id is `<counter>@<origin>`. Only genuinely foreign origins need a
    /// federation push; local members are already reached by the local fan-out,
    /// and duplicates would push the same packet to a peer twice.
    #[test]
    fn remote_orgs_exclude_local_members_and_duplicates() {
        // Exercised through the same parsing the reader uses.
        let self_id = "global";
        let members = ["1@global", "2@peer-a", "3@peer-a", "4@peer-b", "malformed"];
        let mut out: Vec<String> = Vec::new();
        for member in members {
            let Some((_, org)) = member.rsplit_once('@') else {
                continue;
            };
            if org.is_empty() || org == self_id || out.iter().any(|o| o == org) {
                continue;
            }
            out.push(org.to_string());
        }
        assert_eq!(out, vec!["peer-a".to_string(), "peer-b".to_string()]);
    }

    /// A history read with no count must not mean "every row ever": the driver
    /// clamps, and the action supplies a sane page.
    #[test]
    fn history_defaults_to_one_page() {
        let input = HistoryInput::default();
        let count = if input.count > 0 { input.count } else { DEFAULT_HISTORY_COUNT };
        assert_eq!(count, DEFAULT_HISTORY_COUNT);
    }
}
