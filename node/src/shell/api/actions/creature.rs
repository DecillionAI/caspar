//! Translation of `shell/api/actions/creature/creature.go`.
//!
//! Registers every creature lifecycle action against the actor and translates
//! the Go bodies one-to-one. The only deliberate gap is the production
//! Firebase-Auth path inside `/creatures/login`: the Rust workspace does not
//! wire a Firebase SDK, so this port mirrors the Go DEV-mode fallback only
//! (treat `emailToken` as the raw email, or fall back to `username@dev.local`
//! if blank). Wiring the real Firebase verifier is a follow-up.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use base64::Engine;
use chrono::Utc;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::core::actor::model::base::info::Info as BaseInfo;
use crate::core::actor::model::secured::guard::Guard;
use crate::core::actor::model::state::State as ActorState;
use crate::models::action::ExtendedField;
use crate::models::action::ISecureAction;
use crate::models::core::ICore;
use crate::models::input::IInput;
use crate::models::state::IState;
use crate::models::transaction::object_to_map;
use crate::models::transaction::ITrx;
use crate::shell::api::model::{Creature, Session, Store};
use crate::shell::api::packets::creatures::{
    AuthenticateInput, AuthenticateOutput, CheckSignInput, ConsumeLockInput, CreateHoldInput,
    CreateInput as CreatureCreateInput, DeleteInput, FindInput, GetByUsernameInput,
    GetFinancialAccountInput, GetHoldInput, GetInput, GetOutput, ListInput, LockTokenInput,
    ReconcileFinancialSystemInput, RegisterFinanceNodeInput, RegisterFinanceResourceInput,
    RequestPayoutInput, ResolvePayoutInput, RetireFinanceNodeInput, RetireFinanceResourceInput,
    ReviewFinanceResourceInput, ListPayoutsInput, LoginInput, LoginOutput, MetaInput, MintInput,
    PaymentAdjustmentInput, PublishFinanceCatalogInput, PublishFinanceQuoteInput, ReleaseHoldInput,
    SecretGetInput, SecretGrantInput, SecretListGrantedInput, SecretListInput, SecretPutInput,
    SecretRevokeInput, SettleHoldInput, SignalInput as CreatureSignalInput, StartHoldInput,
    StorageUploadInput, TransferInput, UpdateInput,
};
use crate::shell::api::packets::stores::Send as StoresSend;
use crate::shell::utils::crypto::{secure_key_pairs, secure_unique_string};
use crate::shell::utils::future::async_once;
use crate::shell::utils::secret_crypto;

use super::util::build_secure_action;

fn user_guard() -> Guard {
    Guard {
        is_user: true,
        is_in_store: false,
        // Preserve applet authentication for legacy non-financial actions.
        allow_applet_sign: true,
    }
}
fn finance_guard() -> Guard {
    Guard {
        is_user: true,
        is_in_store: false,
        allow_applet_sign: false,
    }
}

fn anon_guard() -> Guard {
    Guard::default()
}

fn as_i64(raw: &Value) -> Option<i64> {
    match raw {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        _ => None,
    }
}

/// Build the `/creatures/create` action.
// ─────────────────────────── Creature type registry ───────────────────────────
//
// A creature is the general model for every being on the network that can act
// (hold a balance, own resources, hold accesses). Its base is fixed — id,
// publicKey, balance, username — and the host system managing Caspar registers
// *customized* creature types on top of that base, each declaring its own
// behaviour (initial balance) and any custom fields.
// Two types are registered out of the box: `human` (the primary being) and
// `machine` (a non-human being that can own programs). Registration is
// idempotent and runs in the install (bootstrap) phase of the shell API.

const DEFAULT_CREATURE_INITIAL_BALANCE: i64 = 0;
const LEGACY_HUMAN_INITIAL_BALANCE: i64 = 1_000_000_000_000_000;

fn creature_type_key(name: &str) -> String {
    format!("Json::CreatureType::{}", name)
}

/// The spec of a registered creature type, if present.
fn get_creature_type(trx: &dyn ITrx, name: &str) -> Option<Map<String, Value>> {
    match trx.get_json(&creature_type_key(name), "spec") {
        Ok(m) if !m.is_empty() => Some(m),
        _ => None,
    }
}

/// Register a creature type only if it does not already exist (idempotent).
fn register_creature_type_if_absent(trx: &dyn ITrx, name: &str, spec: Value) {
    if get_creature_type(trx, name).is_some() {
        return;
    }
    if spec.is_object() {
        let _ = trx.put_json(&creature_type_key(name), "spec", &spec, false);
        trx.put_link(&format!("CreatureTypeExists::{}", name), "true");
    }
}

/// Replace the old built-in human grant without overwriting a host-defined
/// balance. Nodes that already installed the human type otherwise retain the
/// legacy value forever because built-in type registration is idempotent.
fn migrate_legacy_human_balance(trx: &dyn ITrx) {
    let Some(mut spec) = get_creature_type(trx, "human") else {
        return;
    };
    if spec.get("initialBalance").and_then(Value::as_i64) != Some(LEGACY_HUMAN_INITIAL_BALANCE) {
        return;
    }

    spec.insert(
        "initialBalance".to_string(),
        json!(DEFAULT_CREATURE_INITIAL_BALANCE),
    );
    let _ = trx.put_json(
        &creature_type_key("human"),
        "spec",
        &Value::Object(spec),
        false,
    );
}

/// Idempotently register the built-in creature types. Safe to call from every
/// namespace's `install()` — the host can register additional custom types the
/// same way.
pub fn install_creature_types(app: Arc<dyn ICore>) {
    app.modify_state(
        false,
        Box::new(|trx: &dyn ITrx| {
            register_creature_type_if_absent(
                trx,
                "human",
                json!({
                    "initialBalance": DEFAULT_CREATURE_INITIAL_BALANCE,
                    "customFields": [],
                    "desc": "The primary human being on the network."
                }),
            );
            register_creature_type_if_absent(
                trx,
                "machine",
                json!({
                    "initialBalance": DEFAULT_CREATURE_INITIAL_BALANCE,
                    "customFields": [],
                    "desc": "A non-human being that can own programs."
                }),
            );
            migrate_legacy_human_balance(trx);
            Ok(())
        }),
    );
}

/// Resolve a creature type's initial balance from the registry. Falls back to
/// the built-in seed values when the registry has not been seeded yet (the very
/// first creature is created before `install` runs), and rejects unknown types.
fn resolve_initial_balance(trx: &dyn ITrx, creature_type: &str) -> Result<i64> {
    match get_creature_type(trx, creature_type) {
        Some(spec) => Ok(spec
            .get("initialBalance")
            .and_then(|v| v.as_i64())
            .unwrap_or(0)),
        None => match creature_type {
            "human" | "machine" => Ok(DEFAULT_CREATURE_INITIAL_BALANCE),
            other => Err(anyhow!("unknown creature type: {}", other)),
        },
    }
}

fn create(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    let app_for_handler = app.clone();
    build_secure_action::<CreatureCreateInput, _>(
        app,
        "/creatures/create",
        anon_guard(),
        move |state: Arc<dyn IState>, input: CreatureCreateInput| -> Result<Value> {
            let trx = state.trx();
            let creature_type = input.typ.clone();
            let username = format!("{}@{}", input.username, state.source());
            let mut chain_id = "main".to_string();
            let mut subchain_id = "main".to_string();
            let mut owner_id = "free".to_string();
            if let Some(s) = input.chain_id.as_ref() {
                if !s.is_empty() {
                    chain_id = s.clone();
                }
            }
            if let Some(s) = input.subchain_id.as_ref() {
                if !s.is_empty() {
                    subchain_id = s.clone();
                }
            }
            if let Some(s) = input.owner_id.as_ref() {
                if !s.is_empty() {
                    owner_id = s.clone();
                }
            }
            if creature_type == "human" {
                chain_id = "main".to_string();
                subchain_id = "main".to_string();
                owner_id = "free".to_string();
            } else if owner_id == "free" {
                owner_id = state.info().user_id();
            }
            if trx.has_index("Creature", "username", "id", &username) {
                return Err(anyhow!("creature username already exists"));
            }
            // Initial balance comes from the registered creature type.
            let balance = resolve_initial_balance(&*trx, &creature_type)?;
            let creature = Creature {
                id: app_for_handler
                    .tools()
                    .storage()
                    .gen_id(&*trx, &input.origin()),
                type_name: creature_type.clone(),
                username: username.clone(),
                public_key: input.public_key.clone(),
                chain_id,
                subchain_id,
                owner_id: owner_id.clone(),
                balance,
                ..Default::default()
            };
            // Creature is the single record for every being — identity, balance,
            // and program ownership all live here. No separate User/Machine rows.
            creature.push(&*trx);
            let session = Session {
                id: app_for_handler
                    .tools()
                    .storage()
                    .gen_id(&*trx, &input.origin()),
                user_id: creature.id.clone(),
            };
            session.push(&*trx);
            let _ = trx.put_json(
                &format!("CreatMeta::{}", creature.id),
                "metadata",
                &input.metadata,
                false,
            );
            let _ = trx.put_json(
                &format!("UserMeta::{}", creature.id),
                "metadata",
                &input.metadata,
                false,
            );
            if creature_type != "human" {
                trx.put_link(&format!("ownerof::{}::{}", owner_id, creature.id), "true");
            }
            Ok(json!({"creature": creature, "session": session}))
        },
    )
}

fn get(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<GetInput, _>(
        app,
        "/creatures/get",
        user_guard(),
        move |state: Arc<dyn IState>, input: GetInput| -> Result<Value> {
            let trx = state.trx();
            if trx.has_obj("Creature", &input.user_id) {
                let creature = Creature {
                    id: input.user_id.clone(),
                    ..Default::default()
                }
                .pull(&*trx);
                return Ok(json!({"creature": creature}));
            }
            Err(anyhow!("creature not found"))
        },
    )
}

fn list(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<ListInput, _>(
        app,
        "/creatures/list",
        user_guard(),
        move |state: Arc<dyn IState>, input: ListInput| -> Result<Value> {
            let creatures = Creature::all(&*state.trx(), input.offset, input.count)?;
            Ok(json!({"creatures": creatures}))
        },
    )
}

fn transfer(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<TransferInput, _>(
        app,
        "/creatures/transfer",
        finance_guard(),
        move |state: Arc<dyn IState>, input: TransferInput| -> Result<Value> {
            let trx = state.trx();
            if input.amount <= 0 {
                return Err(anyhow!("amount must be greater than zero"));
            }
            let mut from = Creature {
                id: state.info().user_id(),
                ..Default::default()
            }
            .pull(&*trx);
            if from.id.is_empty() {
                return Err(anyhow!("sender creature not found"));
            }
            if from.balance < input.amount {
                return Err(anyhow!("your balance is not enough"));
            }
            let to_id = trx.get_index("Creature", "username", "id", &input.to_username);
            if to_id.is_empty() {
                return Err(anyhow!("target creature not found"));
            }
            if to_id == from.id {
                return Err(anyhow!("cannot transfer to the same wallet"));
            }
            let mut to = Creature {
                id: to_id,
                ..Default::default()
            }
            .pull(&*trx);
            if to.id.is_empty() {
                return Err(anyhow!("target creature not found"));
            }
            let from_id = from.id.clone();
            let to_id = to.id.clone();
            let from_withdrawable = finance_withdrawable_amount(&*trx, &from_id)?;
            if from_withdrawable > from.balance {
                return Err(anyhow!("withdrawable balance exceeds available balance"));
            }
            let nonwithdrawable = from.balance - from_withdrawable;
            let sent_withdrawable = input.amount.saturating_sub(nonwithdrawable);
            from.balance = from
                .balance
                .checked_sub(input.amount)
                .ok_or_else(|| anyhow!("sender balance underflow"))?;
            set_finance_withdrawable_amount(
                &*trx,
                &from_id,
                from_withdrawable
                    .checked_sub(sent_withdrawable)
                    .ok_or_else(|| anyhow!("sender withdrawable underflow"))?,
            )?;

            let debt = finance_debt_amount(&*trx, &to_id)?;
            let debt_repaid = debt.min(input.amount);
            let wallet_credit = input.amount - debt_repaid;
            let sent_nonwithdrawable = input.amount - sent_withdrawable;
            let withdrawable_used_for_debt = debt_repaid.saturating_sub(sent_nonwithdrawable);
            let received_withdrawable = sent_withdrawable
                .checked_sub(withdrawable_used_for_debt)
                .ok_or_else(|| anyhow!("target withdrawable underflow"))?;
            to.balance = to
                .balance
                .checked_add(wallet_credit)
                .ok_or_else(|| anyhow!("target balance overflow"))?;
            let to_withdrawable = finance_withdrawable_amount(&*trx, &to_id)?
                .checked_add(received_withdrawable)
                .ok_or_else(|| anyhow!("target withdrawable overflow"))?;
            set_finance_debt_amount(&*trx, &to_id, debt - debt_repaid)?;
            set_finance_withdrawable_amount(&*trx, &to_id, to_withdrawable)?;
            from.push(&*trx);
            to.push(&*trx);
            let now = Utc::now().timestamp_millis();
            let journal_id = write_finance_journal(
                &*trx,
                "wallet.transfer",
                "",
                &from_id,
                json!({
                    "entries": [
                        {"account": format!("wallet:{from_id}:available"), "amount": -input.amount},
                        {"account": format!("wallet:{to_id}:available"), "amount": wallet_credit},
                        {"account": format!("wallet:{to_id}:debt"), "amount": -debt_repaid}
                    ],
                    "amount": input.amount,
                    "withdrawableAmount": sent_withdrawable,
                    "debtRepaid": debt_repaid,
                }),
                &[from_id.clone(), to_id.clone()],
                now,
            )?;
            Ok(json!({
                "amount": input.amount,
                "toUserId": to_id,
                "debtRepaid": debt_repaid,
                "journalId": journal_id,
            }))
        },
    )
}

fn signal(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    let app_for_handler = app.clone();
    build_secure_action::<CreatureSignalInput, _>(
        app,
        "/creatures/signal",
        user_guard(),
        move |state: Arc<dyn IState>, input: CreatureSignalInput| -> Result<Value> {
            let trx = state.trx();
            let sender_creature = Creature {
                id: state.info().user_id(),
                ..Default::default()
            }
            .pull(&*trx);
            // The signal carries the sender's Creature identity; balance is
            // zeroed so it is never leaked over the signalling channel.
            let mut sender = sender_creature.clone();
            sender.balance = 0;
            let store_id = state.info().store_id();
            if input.typ == "all" {
                if store_id.is_empty() {
                    return Err(anyhow!("storeId is required for broadcast"));
                }
                if trx.get_link(&format!(
                    "onaccess::{}::{}",
                    store_id,
                    state.info().user_id()
                )) != "true"
                {
                    return Err(anyhow!("access denied"));
                }
                let packet = StoresSend {
                    action: "broadcast".to_string(),
                    user: sender.clone(),
                    data: input.data.clone(),
                    is_temp: input.temp,
                    ..Default::default()
                };
                let app_async = app_for_handler.clone();
                let store_id_async = store_id.clone();
                let exception_user_id = state.info().user_id();
                let _ = async_once(move || {
                    app_async.tools().signaler().signal_group(
                        "creatures/signal",
                        &store_id_async,
                        serde_json::to_value(&packet).unwrap_or(Value::Null),
                        true,
                        vec![exception_user_id],
                    );
                });
                return Ok(json!({"passed": true}));
            }
            if input.typ != "pvp" {
                return Err(anyhow!("unknown signal type"));
            }
            if input.creature_id.is_empty() {
                return Err(anyhow!("creatureId is required for pvp"));
            }
            let target_id = if !input.program_id.is_empty() {
                input.program_id.clone()
            } else {
                input.creature_id.clone()
            };
            let packet = StoresSend {
                action: "single".to_string(),
                user: sender,
                // Stamp the store this signal was sent within onto the envelope
                // so the target learns which space (store) it came from — carried
                // as signal context, not buried in the payload. A proxy entity
                // relays this through to its backbone, where an agent scopes
                // in-space tool/sub-agent discovery to it. Sourced from the
                // signal's declared `storeId` (the guard is not store-scoped, so
                // `state.info().store_id()` is not populated here). Empty when the
                // signal is not scoped to a store, in which case `store_is_empty`
                // skips the field entirely.
                store: Store {
                    id: input.store_id.clone(),
                    ..Default::default()
                },
                data: input.data.clone(),
                is_temp: input.temp,
                entity_id: input.entity_id.clone(),
                correlation_id: input.correlation_id.clone(),
                ..Default::default()
            };
            let app_async = app_for_handler.clone();
            let _ = async_once(move || {
                app_async.tools().signaler().signal_user(
                    "creatures/signal",
                    &target_id,
                    serde_json::to_value(&packet).unwrap_or(Value::Null),
                    true,
                );
            });
            Ok(json!({"passed": true}))
        },
    )
}

fn authenticate(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    let app_for_handler = app.clone();
    build_secure_action::<AuthenticateInput, _>(
        app,
        "/creatures/authenticate",
        user_guard(),
        move |state: Arc<dyn IState>, _: AuthenticateInput| -> Result<Value> {
            let user_id = state.info().user_id();
            // Re-enter /creatures/get on the same trx with the caller as
            // the target user. Mirrors how the Go path stitched together a
            // fresh State+Info pair off mainstate.NewState.
            let inner_info = Arc::new(BaseInfo::new("", ""));
            let inner_state: Arc<dyn IState> =
                Arc::new(ActorState::new(Some(inner_info), Some(state.trx()), ""));
            let get_action = app_for_handler
                .actor()
                .fetch_action("/creatures/get")
                .ok_or_else(|| anyhow!("/creatures/get not registered"))?;
            let typed_input: Arc<dyn IInput> = Arc::new(GetInput {
                user_id: user_id.clone(),
            });
            let (_code, res) = get_action.act(inner_state, typed_input)?;
            let creature: Creature = res
                .get("creature")
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default();
            let mut user_map: HashMap<String, Value> = HashMap::new();
            user_map.insert("id".to_string(), json!(creature.id));
            user_map.insert("type".to_string(), json!(creature.type_name));
            user_map.insert("username".to_string(), json!(creature.username));
            user_map.insert("publicKey".to_string(), json!(creature.public_key));
            user_map.insert("balance".to_string(), json!(creature.balance));
            Ok(serde_json::to_value(AuthenticateOutput {
                authenticated: true,
                user: user_map,
            })?)
        },
    )
}

fn mint(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<MintInput, _>(
        app,
        "/creatures/mint",
        user_guard(),
        move |state: Arc<dyn IState>, input: MintInput| -> Result<Value> {
            if state.info().user_id() != "1@global" {
                return Err(anyhow!("access denied"));
            }
            if input.amount <= 0 {
                return Err(anyhow!("amount must be greater than zero"));
            }
            let trx = state.trx();

            // Exactly-once, when the caller names the payment it is minting.
            //
            // Minting is the only way tokens come into existence and nothing
            // can claw them back, so a caller interrupted *between* a
            // successful mint and recording that fact has no safe move: retry
            // and the payer is credited twice, don't and they are not credited
            // at all. The marker closes that window — the caller retries with
            // the same key and this handler reports the mint it already
            // applied. Handler writes commit as a single batch (see
            // `TrxWrapper::commit`), so the marker and the balance land
            // together or not at all.
            let marker = match input.idempotency_key.trim() {
                "" => None,
                key => Some(format!("MintApplied::{}", key)),
            };
            if let Some(marker) = &marker {
                let applied = trx.get_link(marker);
                if !applied.is_empty() {
                    return Ok(json!({
                        "applied": false,
                        "alreadyApplied": true,
                        "previous": applied,
                    }));
                }
            }

            // The email→id link resolves the creature directly; credit the
            // single authoritative Creature balance.
            let to_user_id = trx.get_link(&format!("UserEmailToId::{}", input.to_user_email));
            if to_user_id.is_empty() {
                return Err(anyhow!("target user not found"));
            }
            let mut creature = Creature {
                id: to_user_id,
                ..Default::default()
            }
            .pull(&*trx);
            if creature.id.is_empty() || creature.type_name.is_empty() {
                return Err(anyhow!("target user not found"));
            }
            let debt = finance_debt_amount(&*trx, &creature.id)?;
            let debt_repaid = debt.min(input.amount);
            let wallet_credit = input
                .amount
                .checked_sub(debt_repaid)
                .ok_or_else(|| anyhow!("mint credit underflow"))?;
            creature.balance = creature
                .balance
                .checked_add(wallet_credit)
                .ok_or_else(|| anyhow!("balance overflow"))?;
            creature.push(&*trx);
            set_finance_debt_amount(&*trx, &creature.id, debt - debt_repaid)?;
            let target_id = creature.id.clone();
            let participants = vec![target_id.clone(), state.info().user_id()];
            let journal_id = write_finance_journal(
                &*trx,
                "payment.credited",
                "",
                &target_id,
                json!({
                    "entries": [
                        {"account": "external:payments", "amount": -input.amount},
                        {"account": format!("wallet:{target_id}:available"), "amount": wallet_credit},
                        {"account": format!("wallet:{target_id}:debt"), "amount": -debt_repaid}
                    ],
                    "grossAmount": input.amount, "walletCredit": wallet_credit,
                    "debtRepaid": debt_repaid, "paymentReference": input.idempotency_key,
                }),
                &participants,
                Utc::now().timestamp_millis(),
            )?;
            if let Some(marker) = &marker {
                trx.put_link(
                    marker,
                    &format!("{}:{}:{}", target_id, input.amount, journal_id),
                );
            }
            Ok(json!({
                "applied": true, "balance": creature.balance,
                "walletCredit": wallet_credit, "debtRepaid": debt_repaid,
                "debt": debt - debt_repaid, "journalId": journal_id,
            }))
        },
    )
}

fn check_sign(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    let app_for_handler = app.clone();
    build_secure_action::<CheckSignInput, _>(
        app,
        "/creatures/checkSign",
        user_guard(),
        move |state: Arc<dyn IState>, input: CheckSignInput| -> Result<Value> {
            if state.info().user_id() != "1@global" {
                return Err(anyhow!("access denied"));
            }
            let data = match base64::engine::general_purpose::STANDARD.decode(&input.payload) {
                Ok(d) => d,
                Err(e) => {
                    log::warn!("checkSign decode: {}", e);
                    return Ok(json!({"valid": false}));
                }
            };
            let (success, _, _) = app_for_handler.tools().security().auth_with_signature(
                &input.user_id,
                &data,
                &input.signature,
            );
            if success {
                let email = state
                    .trx()
                    .get_link(&format!("UserIdToEmail::{}", input.user_id));
                return Ok(json!({"valid": true, "email": email}));
            }
            Ok(json!({"valid": false}))
        },
    )
}

// ── creature-owned secrets ──────────────────────────────────────────────────
// A creature stores a secret so its value lives on-chain only as ciphertext
// (encrypted under the node master key, off-chain). The owner can always read it
// back; it may grant another creature time-boxed, revocable read access. Access
// control is enforced HERE against the authenticated caller (`user_id()`), which
// a creature cannot forge — the encryption alone is not the boundary.

const SECRET_PREFIX: &str = "Secret::";
const SECRET_GRANT_PREFIX: &str = "SecretGrant::";
const SECRET_GRANTEE_PREFIX: &str = "SecretGrantee::";

fn secret_key(owner: &str, name: &str) -> String {
    format!("{SECRET_PREFIX}{owner}::{name}")
}
fn secret_grant_key(owner: &str, name: &str, grantee: &str) -> String {
    format!("{SECRET_GRANT_PREFIX}{owner}::{name}::{grantee}")
}
/// Reverse index keyed by grantee, so a grantee can enumerate its grants.
fn secret_grantee_key(grantee: &str, owner: &str, name: &str) -> String {
    format!("{SECRET_GRANTEE_PREFIX}{grantee}::{owner}::{name}")
}
/// Names/ids are path components of the storage key, so a ':' would let a caller
/// escape its own namespace. Reject it rather than sanitize silently.
fn valid_component(s: &str) -> bool {
    !s.is_empty() && !s.contains(':')
}

/// The unexpired `{owner, name}` grants held by `grantee`, from the reverse index.
/// Shared by the signed route and the docker host-call so both return the same set.
pub(crate) fn list_granted_secrets(trx: &dyn ITrx, grantee: &str) -> Vec<Value> {
    let prefix = format!("{SECRET_GRANTEE_PREFIX}{grantee}::");
    let now = Utc::now().timestamp_millis();
    let mut out = Vec::new();
    for key in trx.get_by_prefix(&prefix) {
        let Some(rest) = key.strip_prefix(&prefix) else {
            continue;
        };
        // rest = "<owner>::<name>"; owner has no "::" and name has no ':'.
        let Some((owner, name)) = rest.split_once("::") else {
            continue;
        };
        let expires_at: i64 = trx.get_link(&key).trim().parse().unwrap_or(0);
        if expires_at <= 0 || now >= expires_at {
            continue;
        }
        out.push(json!({ "owner": owner, "name": name, "expiresAt": expires_at }));
    }
    out
}

fn secret_put(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    let app_h = app.clone();
    build_secure_action::<SecretPutInput, _>(
        app,
        "/creatures/secretPut",
        user_guard(),
        move |state: Arc<dyn IState>, input: SecretPutInput| -> Result<Value> {
            let owner = state.info().user_id();
            if owner.is_empty() {
                return Err(anyhow!("not authenticated"));
            }
            if !valid_component(&input.name) {
                return Err(anyhow!("secret name is required and must not contain ':'"));
            }
            if input.value.is_empty() {
                return Err(anyhow!("secret value is required"));
            }
            let root = app_h.tools().storage().storage_root().to_string();
            let key = secret_crypto::master_key(&root)?;
            let blob = secret_crypto::encrypt(input.value.as_bytes(), &key)?;
            let trx = state.trx();
            trx.put_link(&secret_key(&owner, &input.name), &blob);
            Ok(json!({ "ok": true, "name": input.name }))
        },
    )
}

fn secret_get(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    let app_h = app.clone();
    build_secure_action::<SecretGetInput, _>(
        app,
        "/creatures/secretGet",
        user_guard(),
        move |state: Arc<dyn IState>, input: SecretGetInput| -> Result<Value> {
            let caller = state.info().user_id();
            if caller.is_empty() {
                return Err(anyhow!("not authenticated"));
            }
            if !valid_component(&input.name) {
                return Err(anyhow!("secret name is required"));
            }
            let owner = if input.owner.is_empty() {
                caller.clone()
            } else {
                input.owner.clone()
            };
            let trx = state.trx();
            // A non-owner caller needs an unexpired grant.
            if owner != caller {
                let raw = trx.get_link(&secret_grant_key(&owner, &input.name, &caller));
                let expires_at: i64 = raw.trim().parse().unwrap_or(0);
                if expires_at <= 0 || Utc::now().timestamp_millis() >= expires_at {
                    return Err(anyhow!("access denied: no valid grant for this secret"));
                }
            }
            let blob = trx.get_link(&secret_key(&owner, &input.name));
            if blob.is_empty() {
                return Err(anyhow!("secret not found"));
            }
            let root = app_h.tools().storage().storage_root().to_string();
            let key = secret_crypto::master_key(&root)?;
            let plaintext = secret_crypto::decrypt(&blob, &key)?;
            let value = String::from_utf8(plaintext)
                .map_err(|_| anyhow!("stored secret is not valid UTF-8"))?;
            Ok(json!({ "ok": true, "owner": owner, "name": input.name, "value": value }))
        },
    )
}

fn secret_grant(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<SecretGrantInput, _>(
        app,
        "/creatures/secretGrant",
        user_guard(),
        move |state: Arc<dyn IState>, input: SecretGrantInput| -> Result<Value> {
            let owner = state.info().user_id();
            if owner.is_empty() {
                return Err(anyhow!("not authenticated"));
            }
            if !valid_component(&input.name) || !valid_component(&input.grantee) {
                return Err(anyhow!(
                    "name and grantee are required and must not contain ':'"
                ));
            }
            if input.ttl_seconds <= 0 {
                return Err(anyhow!("ttlSeconds must be positive"));
            }
            let trx = state.trx();
            // Only the owner of an existing secret may grant access to it.
            if trx.get_link(&secret_key(&owner, &input.name)).is_empty() {
                return Err(anyhow!("secret not found"));
            }
            let expires_at = Utc::now().timestamp_millis() + input.ttl_seconds * 1000;
            trx.put_link(
                &secret_grant_key(&owner, &input.name, &input.grantee),
                &expires_at.to_string(),
            );
            // Reverse index so a grantee can discover what it was granted without
            // knowing the owner up front (secretListGranted). Same expiry value.
            trx.put_link(
                &secret_grantee_key(&input.grantee, &owner, &input.name),
                &expires_at.to_string(),
            );
            Ok(json!({ "ok": true, "grantee": input.grantee, "expiresAt": expires_at }))
        },
    )
}

fn secret_revoke(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<SecretRevokeInput, _>(
        app,
        "/creatures/secretRevoke",
        user_guard(),
        move |state: Arc<dyn IState>, input: SecretRevokeInput| -> Result<Value> {
            let owner = state.info().user_id();
            if owner.is_empty() {
                return Err(anyhow!("not authenticated"));
            }
            if !valid_component(&input.name) || !valid_component(&input.grantee) {
                return Err(anyhow!("name and grantee are required"));
            }
            let trx = state.trx();
            trx.del_key(&secret_grant_key(&owner, &input.name, &input.grantee));
            trx.del_key(&secret_grantee_key(&input.grantee, &owner, &input.name));
            Ok(json!({ "ok": true }))
        },
    )
}

/// List the secrets granted TO the caller (as `{owner, name}` pairs), skipping
/// expired grants. Lets a grantee — e.g. the agent backbone — discover the
/// platform secrets it may read without a hardcoded owner. Names/values are not
/// returned; the caller reads each with `secretGet`.
fn secret_list_granted(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<SecretListGrantedInput, _>(
        app,
        "/creatures/secretListGranted",
        user_guard(),
        move |state: Arc<dyn IState>, _input: SecretListGrantedInput| -> Result<Value> {
            let caller = state.info().user_id();
            if caller.is_empty() {
                return Err(anyhow!("not authenticated"));
            }
            let grants = list_granted_secrets(&*state.trx(), &caller);
            Ok(json!({ "ok": true, "grants": grants }))
        },
    )
}

fn secret_list(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<SecretListInput, _>(
        app,
        "/creatures/secretList",
        user_guard(),
        move |state: Arc<dyn IState>, _input: SecretListInput| -> Result<Value> {
            let owner = state.info().user_id();
            if owner.is_empty() {
                return Err(anyhow!("not authenticated"));
            }
            let prefix = format!("{SECRET_PREFIX}{owner}::");
            let names: Vec<String> = state
                .trx()
                .get_by_prefix(&prefix)
                .into_iter()
                .filter_map(|k| k.strip_prefix(&prefix).map(|s| s.to_string()))
                .collect();
            Ok(json!({ "ok": true, "names": names }))
        },
    )
}

/// Upload a file to the node's public blob storage, authenticated as the caller.
/// The bytes live OFF-chain (under `<storage_root>/public-files`) and only the
/// returned id is meant to go on-chain (e.g. an avatar id in a profile). Served
/// back publicly by the storage HTTP endpoint `GET /storage/file/<id>`. An owner
/// sidecar records who uploaded it, so a file is a user-owned entity.
fn storage_upload(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    let app_h = app.clone();
    build_secure_action::<StorageUploadInput, _>(
        app,
        "/storage/upload",
        user_guard(),
        move |state: Arc<dyn IState>, input: StorageUploadInput| -> Result<Value> {
            let owner = state.info().user_id();
            if owner.is_empty() {
                return Err(anyhow!("not authenticated"));
            }
            let data = base64::engine::general_purpose::STANDARD
                .decode(input.data_base64.trim())
                .map_err(|_| anyhow!("dataBase64 is not valid base64"))?;
            if data.is_empty() {
                return Err(anyhow!("empty file"));
            }
            const MAX: usize = 10 * 1024 * 1024; // same bound as the storage HTTP endpoint
            if data.len() > MAX {
                return Err(anyhow!("file too large (max {MAX} bytes)"));
            }
            let ctype = {
                let c = input.content_type.trim();
                if c.is_empty() {
                    "application/octet-stream".to_string()
                } else {
                    c.to_string()
                }
            };
            let root = format!("{}/public-files", app_h.tools().storage().storage_root());
            let id = uuid::Uuid::new_v4().to_string();
            let file = app_h.tools().file();
            file.save_data_to_global_storage(&root, &data, &id, true)
                .map_err(|e| anyhow!("storage write failed: {e}"))?;
            // Sidecars: content type (so the download round-trips it) + owner.
            let _ = file.save_data_to_global_storage(
                &root,
                ctype.as_bytes(),
                &format!("{id}.type"),
                true,
            );
            let _ = file.save_data_to_global_storage(
                &root,
                owner.as_bytes(),
                &format!("{id}.owner"),
                true,
            );
            Ok(json!({ "ok": true, "id": id, "contentType": ctype }))
        },
    )
}
const FINANCE_HOLD_MAX_TTL_MS: i64 = 24 * 60 * 60 * 1000;
// A federated run may pay one agent creator/provider/platform, its execution
// node, and distinct owner/node pairs for every attached tool. Keep this bound
// finite for transaction size while allowing the quoted eight-tool maximum to
// span independent nodes.
const FINANCE_MAX_BENEFICIARIES: usize = 64;

fn valid_finance_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'@'))
}

fn valid_finance_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn valid_finance_origin(value: &str) -> bool {
    if valid_finance_id(value) {
        return true;
    }
    if value.is_empty()
        || value.len() > 2_048
        || value
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return false;
    }
    let Ok(origin) = url::Url::parse(value) else {
        return false;
    };
    matches!(origin.scheme(), "http" | "https" | "ws" | "wss")
        && origin.host_str().is_some()
        && origin.username().is_empty()
        && origin.password().is_none()
        && matches!(origin.path(), "" | "/")
        && origin.query().is_none()
        && origin.fragment().is_none()
}

#[cfg(test)]
mod finance_origin_tests {
    use super::valid_finance_origin;

    #[test]
    fn accepts_caspar_endpoint_origins_and_legacy_ids() {
        for origin in [
            "global",
            "http://localhost:8074",
            "https://node.example:8076",
            "ws://127.0.0.1:8074/",
            "wss://node.example",
        ] {
            assert!(valid_finance_origin(origin), "expected valid origin: {origin}");
        }
    }

    #[test]
    fn rejects_unsafe_or_non_base_origins() {
        for origin in [
            "",
            "ftp://node.example",
            "http://",
            "http://user@node.example",
            "http://node.example/path",
            "http://node.example?query=1",
            "http://node.example#fragment",
        ] {
            assert!(!valid_finance_origin(origin), "expected invalid origin: {origin}");
        }
    }
}

fn finance_hash(value: &Value) -> Result<String> {
    let bytes = serde_json::to_vec(value)?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(hex::encode(hasher.finalize()))
}
fn finance_beneficiary_plan_hash(
    beneficiaries: &[crate::shell::api::packets::creatures::HoldBeneficiaryInput],
) -> String {
    let mut hasher = Sha256::new();
    for beneficiary in beneficiaries {
        hasher.update(beneficiary.user_id.as_bytes());
        hasher.update([0]);
        hasher.update(beneficiary.role.as_bytes());
        hasher.update([0]);
        hasher.update(beneficiary.max_amount.to_string().as_bytes());
        hasher.update([b'\n']);
    }
    hex::encode(hasher.finalize())
}

fn finance_hold_key(hold_id: &str) -> String {
    format!("Json::FinanceHold::{hold_id}")
}

fn get_finance_hold(trx: &dyn ITrx, hold_id: &str) -> Result<Map<String, Value>> {
    trx.get_json(&finance_hold_key(hold_id), "hold")
        .map_err(|_| anyhow!("hold not found"))
}

fn put_finance_hold(trx: &dyn ITrx, hold_id: &str, hold: &Map<String, Value>) -> Result<()> {
    trx.put_json(
        &finance_hold_key(hold_id),
        "hold",
        &Value::Object(hold.clone()),
        false,
    )
}

fn finance_held_amount(trx: &dyn ITrx, payer_id: &str) -> Result<i64> {
    let raw = trx.get_link(&format!("FinanceHeld::{payer_id}"));
    if raw.is_empty() {
        return Ok(0);
    }
    let amount = raw
        .parse::<i64>()
        .map_err(|_| anyhow!("invalid held balance"))?;
    if amount < 0 {
        return Err(anyhow!("invalid held balance"));
    }
    Ok(amount)
}

fn set_finance_held_amount(trx: &dyn ITrx, payer_id: &str, amount: i64) -> Result<()> {
    if amount < 0 {
        return Err(anyhow!("held balance underflow"));
    }
    trx.put_link(&format!("FinanceHeld::{payer_id}"), &amount.to_string());
    Ok(())
}

fn finance_debt_amount(trx: &dyn ITrx, user_id: &str) -> Result<i64> {
    let raw = trx.get_link(&format!("FinanceDebt::{user_id}"));
    if raw.is_empty() {
        return Ok(0);
    }
    let amount = raw
        .parse::<i64>()
        .map_err(|_| anyhow!("invalid wallet debt"))?;
    if amount < 0 {
        return Err(anyhow!("invalid wallet debt"));
    }
    Ok(amount)
}

fn set_finance_debt_amount(trx: &dyn ITrx, user_id: &str, amount: i64) -> Result<()> {
    if amount < 0 {
        return Err(anyhow!("wallet debt underflow"));
    }
    trx.put_link(&format!("FinanceDebt::{user_id}"), &amount.to_string());
    Ok(())
}

fn finance_withdrawable_amount(trx: &dyn ITrx, user_id: &str) -> Result<i64> {
    finance_counter(trx, &format!("FinanceWithdrawable::{user_id}"))
}

fn set_finance_withdrawable_amount(trx: &dyn ITrx, user_id: &str, amount: i64) -> Result<()> {
    if amount < 0 {
        return Err(anyhow!("withdrawable balance underflow"));
    }
    trx.put_link(&format!("FinanceWithdrawable::{user_id}"), &amount.to_string());
    Ok(())
}

fn finance_payout_held_amount(trx: &dyn ITrx, user_id: &str) -> Result<i64> {
    finance_counter(trx, &format!("FinancePayoutHeld::{user_id}"))
}

fn set_finance_payout_held_amount(trx: &dyn ITrx, user_id: &str, amount: i64) -> Result<()> {
    if amount < 0 {
        return Err(anyhow!("payout held balance underflow"));
    }
    trx.put_link(&format!("FinancePayoutHeld::{user_id}"), &amount.to_string());
    Ok(())
}

fn finance_counter(trx: &dyn ITrx, key: &str) -> Result<i64> {
    let raw = trx.get_link(key);
    if raw.is_empty() {
        return Ok(0);
    }
    let value = raw
        .parse::<i64>()
        .map_err(|_| anyhow!("invalid finance counter"))?;
    if value < 0 {
        return Err(anyhow!("invalid finance counter"));
    }
    Ok(value)
}

fn add_finance_counter(trx: &dyn ITrx, key: &str, amount: i64) -> Result<i64> {
    if amount < 0 {
        return Err(anyhow!("finance counter amount must be nonnegative"));
    }
    let next = finance_counter(trx, key)?
        .checked_add(amount)
        .ok_or_else(|| anyhow!("finance counter overflow"))?;
    trx.put_link(key, &next.to_string());
    Ok(next)
}

fn finance_project_budget_key(project_id: &str) -> String {
    format!("Json::FinanceProjectBudget::{project_id}")
}

fn finance_nonnegative_field(map: &Map<String, Value>, field: &str) -> Result<i64> {
    let Some(value) = map.get(field) else {
        return Ok(0);
    };
    let amount = value
        .as_i64()
        .ok_or_else(|| anyhow!("invalid project budget field: {field}"))?;
    if amount < 0 {
        return Err(anyhow!("invalid project budget field: {field}"));
    }
    Ok(amount)
}

fn reserve_project_budget(trx: &dyn ITrx, project_id: &str, amount: i64, now: i64) -> Result<()> {
    if project_id.is_empty() {
        return Ok(());
    }
    if !trx.has_obj(Store::type_(), project_id) {
        return Err(anyhow!("project not found"));
    }
    let metadata = trx
        .get_json(&format!("StoreMeta::{project_id}"), "metadata")
        .unwrap_or_default();
    let configured_budget = finance_nonnegative_field(&metadata, "budgetMinor")?;
    let metadata_spent = finance_nonnegative_field(&metadata, "spentMinor")?;
    let mut state = trx
        .get_json(&finance_project_budget_key(project_id), "budget")
        .unwrap_or_default();
    let ledger_spent = finance_nonnegative_field(&state, "spentMinor")?;
    let spent = ledger_spent.max(metadata_spent);
    let reserved = finance_nonnegative_field(&state, "reservedMinor")?;
    let committed = spent
        .checked_add(reserved)
        .and_then(|value| value.checked_add(amount))
        .ok_or_else(|| anyhow!("project budget overflow"))?;
    if configured_budget > 0 && committed > configured_budget {
        return Err(anyhow!("project budget exceeded"));
    }
    state.insert("projectId".to_string(), json!(project_id));
    state.insert("budgetMinor".to_string(), json!(configured_budget));
    state.insert("spentMinor".to_string(), json!(spent));
    state.insert(
        "reservedMinor".to_string(),
        json!(reserved
            .checked_add(amount)
            .ok_or_else(|| anyhow!("project reservation overflow"))?),
    );
    state.insert("updatedAt".to_string(), json!(now));
    trx.put_json(
        &finance_project_budget_key(project_id),
        "budget",
        &Value::Object(state),
        false,
    )
}

fn finalize_project_budget(
    trx: &dyn ITrx,
    project_id: &str,
    reserved_amount: i64,
    spent_amount: i64,
    now: i64,
) -> Result<()> {
    if project_id.is_empty() {
        return Ok(());
    }
    let mut state = trx
        .get_json(&finance_project_budget_key(project_id), "budget")
        .map_err(|_| anyhow!("project budget reservation not found"))?;
    let reserved = finance_nonnegative_field(&state, "reservedMinor")?;
    let spent = finance_nonnegative_field(&state, "spentMinor")?;
    let remaining = reserved
        .checked_sub(reserved_amount)
        .ok_or_else(|| anyhow!("project budget reservation underflow"))?;
    let total_spent = spent
        .checked_add(spent_amount)
        .ok_or_else(|| anyhow!("project spend overflow"))?;
    state.insert("reservedMinor".to_string(), json!(remaining));
    state.insert("spentMinor".to_string(), json!(total_spent));
    state.insert("updatedAt".to_string(), json!(now));
    trx.put_json(
        &finance_project_budget_key(project_id),
        "budget",
        &Value::Object(state),
        false,
    )
}

fn write_finance_journal(
    trx: &dyn ITrx,
    kind: &str,
    hold_id: &str,
    payer_id: &str,
    payload: Value,
    participants: &[String],
    now: i64,
) -> Result<String> {
    let journal_id = secure_unique_string();
    let entry = json!({
        "journalId": journal_id,
        "kind": kind,
        "holdId": hold_id,
        "payerUserId": payer_id,
        "createdAt": now,
        "payload": payload,
    });
    trx.put_json(
        &format!("Json::FinanceJournal::{journal_id}"),
        "entry",
        &entry,
        false,
    )?;

    let mut indexed: HashMap<&str, bool> = HashMap::new();
    for participant in participants {
        if participant.is_empty() || indexed.insert(participant.as_str(), true).is_some() {
            continue;
        }
        trx.put_link(
            &format!("FinanceJournalByUser::{participant}::{now:020}::{journal_id}"),
            &journal_id,
        );
    }
    Ok(journal_id)
}

const FEDERATED_FINANCE_MAX_RECORD_BYTES: usize = 256 * 1024;

fn federated_finance_object(value: Value, label: &str) -> Result<Map<String, Value>> {
    let object = value
        .as_object()
        .cloned()
        .ok_or_else(|| anyhow!("{label} must be an object"))?;
    if serde_json::to_vec(&object)?.len() > FEDERATED_FINANCE_MAX_RECORD_BYTES {
        return Err(anyhow!("{label} is too large"));
    }
    Ok(object)
}

fn federated_finance_safe_numbers(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(_) | Value::String(_) => true,
        Value::Number(number) => number
            .as_i64()
            .map(|n| (0..=9_007_199_254_740_991).contains(&n))
            .unwrap_or(false),
        Value::Array(values) => values.iter().all(federated_finance_safe_numbers),
        Value::Object(values) => values.values().all(federated_finance_safe_numbers),
    }
}

fn federated_finance_market_bucket(kind: &str) -> Option<&'static str> {
    match kind {
        "agent" => Some("agents"),
        "tool" => Some("tools"),
        "frontend" => Some("frontends"),
        _ => None,
    }
}

fn publish_finance_catalog(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<PublishFinanceCatalogInput, _>(
        app,
        "/creatures/publishFinanceCatalog",
        finance_guard(),
        move |state: Arc<dyn IState>, input: PublishFinanceCatalogInput| -> Result<Value> {
            let trx = state.trx();
            let caller = state.info().user_id();
            if caller != "1@global" {
                return Err(anyhow!("global platform owner required"));
            }
            let catalog = federated_finance_object(input.catalog, "finance catalog")?;
            let version = catalog
                .get("version")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if !valid_finance_id(&version)
                || !federated_finance_safe_numbers(&Value::Object(catalog.clone()))
            {
                return Err(anyhow!("invalid finance catalog"));
            }
            for key in [
                "tokenScale",
                "defaultInputPerMillionMinor",
                "defaultOutputPerMillionMinor",
                "sandboxPerMinuteMinor",
                "minChargeMinor",
                "platformCommissionBps",
                "authorizationSafetyBps",
                "quoteTtlMs",
                "holdTtlMs",
            ] {
                if catalog.get(key).and_then(Value::as_i64).is_none() {
                    return Err(anyhow!("invalid finance catalog integer: {key}"));
                }
            }
            if catalog.get("tokenScale").and_then(Value::as_i64).unwrap_or(0) <= 0
                || catalog
                    .get("sandboxPerMinuteMinor")
                    .and_then(Value::as_i64)
                    .unwrap_or(0)
                    <= 0
            {
                return Err(anyhow!("tokenScale and sandbox rate must be positive"));
            }
            let commission = catalog
                .get("platformCommissionBps")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let safety = catalog
                .get("authorizationSafetyBps")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let quote_ttl = catalog.get("quoteTtlMs").and_then(Value::as_i64).unwrap_or(0);
            let hold_ttl = catalog.get("holdTtlMs").and_then(Value::as_i64).unwrap_or(0);
            if commission > 10_000
                || !(10_000..=100_000).contains(&safety)
                || quote_ttl > hold_ttl
                || hold_ttl > FINANCE_HOLD_MAX_TTL_MS
            {
                return Err(anyhow!("invalid finance catalog policy"));
            }
            for key in [
                "settlementAuthority",
                "platformAccountId",
                "providerClearingAccountId",
                "nodeOwnerAccountId",
            ] {
                let account = catalog.get(key).and_then(Value::as_str).unwrap_or("");
                if !valid_finance_id(account) || !trx.has_obj("Creature", account) {
                    return Err(anyhow!("invalid finance catalog account: {key}"));
                }
            }
            let catalog_value = Value::Object(catalog.clone());
            let catalog_hash = finance_hash(&catalog_value)?;
            let key = format!("Json::BillingCatalog::{version}");
            let mut already_published = false;
            if let Ok(existing) = trx.get_json(&key, "catalog") {
                if !existing.is_empty() {
                    if Value::Object(existing.clone()) != catalog_value {
                        return Err(anyhow!("pricing version is immutable"));
                    }
                    already_published = true;
                }
            }
            trx.put_json(&key, "catalog", &catalog_value, false)?;
            trx.put_json(
                "Json::CreatureNamespace::billing",
                "current",
                &json!({"version": version, "catalogHash": catalog_hash}),
                false,
            )?;
            Ok(json!({
                "ok": true,
                "catalog": catalog,
                "catalogHash": catalog_hash,
                "alreadyPublished": already_published,
            }))
        },
    )
}

fn register_finance_node(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<RegisterFinanceNodeInput, _>(
        app,
        "/creatures/registerFinanceNode",
        finance_guard(),
        move |state: Arc<dyn IState>, input: RegisterFinanceNodeInput| -> Result<Value> {
            let trx = state.trx();
            let caller = state.info().user_id();
            let mut node = federated_finance_object(input.node, "finance node")?;
            let owner = node
                .get("nodeOwnerAccountId")
                .and_then(Value::as_str)
                .unwrap_or("");
            let authority = node
                .get("settlementAuthority")
                .and_then(Value::as_str)
                .unwrap_or("");
            let origin = node.get("originId").and_then(Value::as_str).unwrap_or("");
            let meter = node
                .get("meterProgramId")
                .and_then(Value::as_str)
                .unwrap_or("");
            let talent_meter = node
                .get("talentMeterProgramId")
                .and_then(Value::as_str)
                .unwrap_or("");
            let meter_creature = node
                .get("meterCreatureId")
                .and_then(Value::as_str)
                .unwrap_or("");
            let meter_entity = node
                .get("meterEntityId")
                .and_then(Value::as_str)
                .unwrap_or("");
            let talent_meter_creature = node
                .get("talentMeterCreatureId")
                .and_then(Value::as_str)
                .unwrap_or("");
            let talent_meter_entity = node
                .get("talentMeterEntityId")
                .and_then(Value::as_str)
                .unwrap_or("");
            let revision = node.get("revision").and_then(Value::as_str).unwrap_or("");
            let rate = node
                .get("sandboxPerMinuteMinor")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            if caller != owner
                || authority != caller
                || !valid_finance_id(&caller)
                || !valid_finance_origin(origin)
                || !valid_finance_id(meter)
                || !valid_finance_id(talent_meter)
                || !valid_finance_id(meter_creature)
                || !valid_finance_id(meter_entity)
                || !valid_finance_id(talent_meter_creature)
                || !valid_finance_id(talent_meter_entity)
                || !valid_finance_hash(revision)
                || rate <= 0
                || rate > 9_007_199_254_740_991
                || !trx.has_obj("Creature", &caller)
            {
                return Err(anyhow!("invalid host-attested finance node registration"));
            }
            let now = Utc::now().timestamp_millis();
            node.insert("status".into(), json!("active"));
            node.insert("updatedAt".into(), json!(now));
            trx.put_json(
                "Json::CreatureNamespace::billing",
                "nodes",
                &json!({caller.clone(): Value::Object(node.clone())}),
                true,
            )?;
            Ok(json!({"ok": true, "node": node}))
        },
    )
}

fn retire_finance_node(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<RetireFinanceNodeInput, _>(
        app,
        "/creatures/retireFinanceNode",
        finance_guard(),
        move |state: Arc<dyn IState>, input: RetireFinanceNodeInput| -> Result<Value> {
            let trx = state.trx();
            let caller = state.info().user_id();
            if input.node_owner_account_id != caller || !valid_finance_id(&caller) {
                return Err(anyhow!("node owner mismatch"));
            }
            let nodes = trx
                .get_json("Json::CreatureNamespace::billing", "nodes")
                .unwrap_or_default();
            let mut node = nodes
                .get(&caller)
                .and_then(Value::as_object)
                .cloned()
                .ok_or_else(|| anyhow!("finance node not found"))?;
            let now = Utc::now().timestamp_millis();
            node.insert("status".into(), json!("retired"));
            node.insert("updatedAt".into(), json!(now));
            node.insert("revision".into(), json!(finance_hash(&json!({
                "prior": node.get("revision"), "status": "retired", "updatedAt": now
            }))?));
            trx.put_json(
                "Json::CreatureNamespace::billing",
                "nodes",
                &json!({caller.clone(): Value::Object(node.clone())}),
                true,
            )?;
            Ok(json!({"ok": true, "node": node}))
        },
    )
}

fn register_finance_resource(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<RegisterFinanceResourceInput, _>(
        app,
        "/creatures/registerFinanceResource",
        finance_guard(),
        move |state: Arc<dyn IState>, input: RegisterFinanceResourceInput| -> Result<Value> {
            let trx = state.trx();
            let caller = state.info().user_id();
            let mut resource = federated_finance_object(input.resource, "finance resource")?;
            let resource_id = resource
                .get("programId")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let kind = resource
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let owner = resource
                .get("owner")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let host_owner = resource
                .get("hostNodeOwnerAccountId")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let bucket = federated_finance_market_bucket(&kind)
                .ok_or_else(|| anyhow!("invalid finance resource kind"))?;
            let pricing = resource.get("pricing").cloned().unwrap_or(Value::Null);
            // Program records are execution-node state. The finance host bridge
            // resolves Program -> Machine -> owner locally and overwrites these
            // fields before the node owner signs this global attestation.
            if caller != host_owner
                || !valid_finance_id(&resource_id)
                || !valid_finance_id(&owner)
                || !trx.has_obj("Creature", &owner)
                || !federated_finance_safe_numbers(&pricing)
            {
                return Err(anyhow!("invalid host-attested finance resource"));
            }
            let nodes = trx
                .get_json("Json::CreatureNamespace::billing", "nodes")
                .unwrap_or_default();
            let node = nodes
                .get(&host_owner)
                .and_then(Value::as_object)
                .ok_or_else(|| anyhow!("finance execution node not registered"))?;
            if node.get("status").and_then(Value::as_str) != Some("active")
                || resource.get("hostOriginId").and_then(Value::as_str)
                    != node.get("originId").and_then(Value::as_str)
                || resource.get("billingMeterProgramId").and_then(Value::as_str)
                    != node.get("meterProgramId").and_then(Value::as_str)
                || resource.get("billingMeterCreatureId").and_then(Value::as_str)
                    != node.get("meterCreatureId").and_then(Value::as_str)
                || resource.get("billingMeterEntityId").and_then(Value::as_str)
                    != node.get("meterEntityId").and_then(Value::as_str)
                || resource.get("nodeRegistrationRevision").and_then(Value::as_str)
                    != node.get("revision").and_then(Value::as_str)
                || resource.get("nodeSandboxPerMinuteMinor").and_then(Value::as_i64)
                    != node.get("sandboxPerMinuteMinor").and_then(Value::as_i64)
            {
                return Err(anyhow!("resource does not match its active finance node"));
            }
            let entries = trx
                .get_json("Json::CreatureNamespace::market", bucket)
                .unwrap_or_default();
            let existing = entries.get(&resource_id).and_then(Value::as_object);
            if let Some(existing) = existing {
                if existing
                    .get("hostNodeOwnerAccountId")
                    .and_then(Value::as_str)
                    != Some(host_owner.as_str())
                {
                    return Err(anyhow!("resource migration requires a new program id"));
                }
            }
            let requested_status = resource
                .get("status")
                .and_then(Value::as_str)
                .filter(|status| caller == "1@global" && matches!(*status, "approved" | "denied"))
                .unwrap_or("pending");
            let preserved_status = existing
                .and_then(|row| row.get("status"))
                .and_then(Value::as_str)
                .filter(|status| matches!(*status, "approved" | "denied"))
                .unwrap_or(requested_status);
            resource.insert("status".into(), json!(preserved_status));
            resource.insert("federated".into(), json!(true));
            resource.insert("registeredAt".into(), json!(Utc::now().timestamp_millis()));
            trx.put_json(
                "Json::CreatureNamespace::market",
                bucket,
                &json!({resource_id.clone(): Value::Object(resource.clone())}),
                true,
            )?;
            Ok(json!({"ok": true, "resource": resource}))
        },
    )
}

fn review_finance_resource(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<ReviewFinanceResourceInput, _>(
        app,
        "/creatures/reviewFinanceResource",
        finance_guard(),
        move |state: Arc<dyn IState>, input: ReviewFinanceResourceInput| -> Result<Value> {
            let trx = state.trx();
            let caller = state.info().user_id();
            if caller != "1@global" || !matches!(input.status.as_str(), "approved" | "denied") {
                return Err(anyhow!("global finance reviewer required"));
            }
            let bucket = federated_finance_market_bucket(&input.kind)
                .ok_or_else(|| anyhow!("invalid finance resource kind"))?;
            let entries = trx
                .get_json("Json::CreatureNamespace::market", bucket)
                .unwrap_or_default();
            let mut resource = entries
                .get(&input.resource_id)
                .and_then(Value::as_object)
                .cloned()
                .ok_or_else(|| anyhow!("finance resource not found"))?;
            resource.insert("status".into(), json!(input.status));
            resource.insert("reviewedBy".into(), json!(caller));
            resource.insert("reviewedAt".into(), json!(Utc::now().timestamp_millis()));
            if !input.reason.is_empty() {
                resource.insert("reason".into(), json!(input.reason));
            }
            trx.put_json(
                "Json::CreatureNamespace::market",
                bucket,
                &json!({input.resource_id.clone(): Value::Object(resource.clone())}),
                true,
            )?;
            Ok(json!({"ok": true, "resource": resource}))
        },
    )
}

fn retire_finance_resource(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<RetireFinanceResourceInput, _>(
        app,
        "/creatures/retireFinanceResource",
        finance_guard(),
        move |state: Arc<dyn IState>, input: RetireFinanceResourceInput| -> Result<Value> {
            let trx = state.trx();
            let caller = state.info().user_id();
            let bucket = federated_finance_market_bucket(&input.kind)
                .ok_or_else(|| anyhow!("invalid finance resource kind"))?;
            let entries = trx
                .get_json("Json::CreatureNamespace::market", bucket)
                .unwrap_or_default();
            let resource = entries
                .get(&input.resource_id)
                .and_then(Value::as_object)
                .ok_or_else(|| anyhow!("finance resource not found"))?;
            let host_owner = resource
                .get("hostNodeOwnerAccountId")
                .and_then(Value::as_str)
                .unwrap_or("");
            if caller != host_owner && caller != "1@global" {
                return Err(anyhow!("resource host or global reviewer required"));
            }
            trx.put_json(
                "Json::CreatureNamespace::market",
                bucket,
                &json!({input.resource_id.clone(): Value::Null}),
                true,
            )?;
            Ok(json!({"ok": true, "resourceId": input.resource_id}))
        },
    )
}

fn validate_federated_quote_resource(
    trx: &dyn ITrx,
    execution: &Map<String, Value>,
) -> Result<()> {
    let resource_id = execution
        .get("resourceId")
        .and_then(Value::as_str)
        .unwrap_or("");
    let kind = execution.get("kind").and_then(Value::as_str).unwrap_or("");
    let bucket = federated_finance_market_bucket(kind)
        .ok_or_else(|| anyhow!("invalid quote execution resource kind"))?;
    if !valid_finance_id(resource_id) {
        return Err(anyhow!("invalid quote execution resource id"));
    }
    let entries = trx
        .get_json("Json::CreatureNamespace::market", bucket)
        .unwrap_or_default();
    let resource = entries
        .get(resource_id)
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("quoted resource is not globally registered"))?;
    if resource.get("status").and_then(Value::as_str) != Some("approved") {
        return Err(anyhow!("quoted resource is not globally approved"));
    }
    let node_owner = resource
        .get("hostNodeOwnerAccountId")
        .and_then(Value::as_str)
        .unwrap_or("");
    let nodes = trx
        .get_json("Json::CreatureNamespace::billing", "nodes")
        .unwrap_or_default();
    let node = nodes
        .get(node_owner)
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("quoted resource node is not registered"))?;
    if node.get("status").and_then(Value::as_str) != Some("active")
        || execution.get("nodeOwnerAccountId")
            != resource.get("hostNodeOwnerAccountId")
        || execution.get("hostOriginId") != resource.get("hostOriginId")
        || execution.get("meterProgramId") != resource.get("billingMeterProgramId")
        || execution.get("meterCreatureId") != resource.get("billingMeterCreatureId")
        || execution.get("meterEntityId") != resource.get("billingMeterEntityId")
        || execution.get("nodeRegistrationRevision")
            != resource.get("nodeRegistrationRevision")
        || execution.get("sandboxPerMinuteMinor")
            != resource.get("nodeSandboxPerMinuteMinor")
        || resource.get("hostOriginId") != node.get("originId")
        || resource.get("billingMeterProgramId") != node.get("meterProgramId")
        || resource.get("billingMeterCreatureId") != node.get("meterCreatureId")
        || resource.get("billingMeterEntityId") != node.get("meterEntityId")
        || resource.get("nodeRegistrationRevision") != node.get("revision")
        || resource.get("nodeSandboxPerMinuteMinor")
            != node.get("sandboxPerMinuteMinor")
        || execution.get("settlementAuthority") != node.get("settlementAuthority")
    {
        return Err(anyhow!("quote execution does not match the active global resource binding"));
    }
    Ok(())
}

fn publish_finance_quote(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<PublishFinanceQuoteInput, _>(
        app,
        "/creatures/publishFinanceQuote",
        finance_guard(),
        move |state: Arc<dyn IState>, input: PublishFinanceQuoteInput| -> Result<Value> {
            let trx = state.trx();
            let caller = state.info().user_id();
            let mut quote = federated_finance_object(input.quote, "finance quote")?;
            let quote_id = quote
                .get("quoteId")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let payer = quote
                .get("payerUserId")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let max_amount = quote.get("maxAmount").and_then(Value::as_i64).unwrap_or(0);
            let hold = quote
                .get("holdRequest")
                .and_then(Value::as_object)
                .ok_or_else(|| anyhow!("quote holdRequest missing"))?;
            let execution = quote
                .get("executionPlan")
                .and_then(Value::as_object)
                .ok_or_else(|| anyhow!("quote executionPlan missing"))?;
            let quote_kind = quote.get("kind").and_then(Value::as_str).unwrap_or("");
            let authority = execution
                .get("settlementAuthority")
                .and_then(Value::as_str)
                .unwrap_or("");
            let meter = execution
                .get("meterProgramId")
                .and_then(Value::as_str)
                .unwrap_or("");
            let meter_creature = execution
                .get("meterCreatureId")
                .and_then(Value::as_str)
                .unwrap_or("");
            let meter_entity = execution
                .get("meterEntityId")
                .and_then(Value::as_str)
                .unwrap_or("");
            let pricing_version = quote
                .get("pricingVersion")
                .and_then(Value::as_str)
                .unwrap_or("");
            let active_catalog = trx
                .get_json("Json::CreatureNamespace::billing", "current")
                .unwrap_or_default();
            let catalog_exists = !pricing_version.is_empty()
                && trx
                    .get_json(
                        &format!("Json::BillingCatalog::{pricing_version}"),
                        "catalog",
                    )
                    .map(|catalog| !catalog.is_empty())
                    .unwrap_or(false);
            let nodes = trx
                .get_json("Json::CreatureNamespace::billing", "nodes")
                .unwrap_or_default();
            let issuer_node = nodes.get(&caller).and_then(Value::as_object);
            let coordinator_node = nodes.get(authority).and_then(Value::as_object);
            let expected_meter = coordinator_node.and_then(|node| {
                if quote_kind == "talent" {
                    node.get("talentMeterProgramId")
                } else {
                    node.get("meterProgramId")
                }
            });
            if !valid_finance_id(&quote_id)
                || !valid_finance_id(&payer)
                || !matches!(quote_kind, "agent" | "tool" | "talent")
                || active_catalog.get("version").and_then(Value::as_str)
                    != Some(pricing_version)
                || !catalog_exists
                || max_amount <= 0
                || (quote_kind == "talent" && authority != caller)
                || issuer_node.and_then(|node| node.get("status")).and_then(Value::as_str)
                    != Some("active")
                || coordinator_node.and_then(|node| node.get("status")).and_then(Value::as_str)
                    != Some("active")
                || expected_meter.and_then(Value::as_str) != Some(meter)
                || (quote_kind != "talent"
                    && (coordinator_node
                        .and_then(|node| node.get("meterCreatureId"))
                        .and_then(Value::as_str)
                        != Some(meter_creature)
                        || coordinator_node
                            .and_then(|node| node.get("meterEntityId"))
                            .and_then(Value::as_str)
                            != Some(meter_entity)))
                || hold.get("quoteId").and_then(Value::as_str) != Some(quote_id.as_str())
                || hold.get("maxAmount").and_then(Value::as_i64) != Some(max_amount)
                || hold.get("settlementAuthority").and_then(Value::as_str) != Some(authority)
                || hold.get("meterProgramId").and_then(Value::as_str) != Some(meter)
                || !trx.has_obj("Creature", &payer)
                || !trx.has_obj("Creature", &caller)
                || !federated_finance_safe_numbers(&Value::Object(quote.clone()))
            {
                return Err(anyhow!("invalid immutable finance quote"));
            }
            let resources = execution
                .get("resources")
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow!("quote execution resources missing"))?;
            if quote_kind == "talent" {
                if !resources.is_empty() {
                    return Err(anyhow!("talent quote cannot contain execution resources"));
                }
            } else {
                if resources.is_empty() || resources.len() > 9 {
                    return Err(anyhow!("invalid quote execution resource count"));
                }
                let mut seen = HashMap::<String, bool>::new();
                for (index, raw) in resources.iter().enumerate() {
                    let row = raw
                        .as_object()
                        .ok_or_else(|| anyhow!("invalid quote execution resource"))?;
                    validate_federated_quote_resource(&*trx, row)?;
                    let resource_id = row
                        .get("resourceId")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    if seen.insert(resource_id.clone(), true).is_some() {
                        return Err(anyhow!("duplicate quote execution resource"));
                    }
                    if index == 0
                        && (row.get("kind").and_then(Value::as_str) != Some(quote_kind)
                            || resource_id
                                != quote.get("resourceId").and_then(Value::as_str).unwrap_or(""))
                    {
                        return Err(anyhow!("quote coordinator resource mismatch"));
                    }
                }
                let coordinator = resources[0].as_object().unwrap();
                if coordinator.get("settlementAuthority").and_then(Value::as_str)
                    != Some(authority)
                    || coordinator.get("meterProgramId").and_then(Value::as_str) != Some(meter)
                    || coordinator.get("meterCreatureId").and_then(Value::as_str)
                        != Some(meter_creature)
                    || coordinator.get("meterEntityId").and_then(Value::as_str)
                        != Some(meter_entity)
                {
                    return Err(anyhow!("quote coordinator execution mismatch"));
                }
            }
            let beneficiaries = hold
                .get("beneficiaries")
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow!("quote beneficiaries missing"))?;
            if beneficiaries.is_empty() || beneficiaries.len() > FINANCE_MAX_BENEFICIARIES {
                return Err(anyhow!("invalid quote beneficiary count"));
            }
            let mut cap_total = 0_i64;
            for raw in beneficiaries {
                let row = raw.as_object().ok_or_else(|| anyhow!("invalid quote beneficiary"))?;
                let user_id = row.get("userId").and_then(Value::as_str).unwrap_or("");
                let amount = row.get("maxAmount").and_then(Value::as_i64).unwrap_or(0);
                if !valid_finance_id(user_id) || amount <= 0 || !trx.has_obj("Creature", user_id) {
                    return Err(anyhow!("invalid quote beneficiary"));
                }
                cap_total = cap_total
                    .checked_add(amount)
                    .ok_or_else(|| anyhow!("quote beneficiary overflow"))?;
            }
            if cap_total != max_amount {
                return Err(anyhow!("quote caps do not equal maxAmount"));
            }
            let key = format!("Json::BillingQuote::{quote_id}");
            if let Ok(existing) = trx.get_json(&key, "quote") {
                if !existing.is_empty() {
                    let mut comparable = existing.clone();
                    comparable.remove("quoteIssuerNodeOwnerId");
                    comparable.remove("publishedAt");
                    if comparable != quote {
                        return Err(anyhow!("quote id is immutable"));
                    }
                    return Ok(json!({"ok": true, "alreadyPublished": true, "quote": existing}));
                }
            }
            quote.insert("quoteIssuerNodeOwnerId".into(), json!(caller));
            quote.insert("publishedAt".into(), json!(Utc::now().timestamp_millis()));
            trx.put_json(&key, "quote", &Value::Object(quote.clone()), false)?;
            Ok(json!({"ok": true, "quote": quote}))
        },
    )
}

fn create_hold(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    let security_app = app.clone();
    build_secure_action::<CreateHoldInput, _>(
        app,
        "/creatures/createHold",
        finance_guard(),
        move |state: Arc<dyn IState>, input: CreateHoldInput| -> Result<Value> {
            let trx = state.trx();
            let payer_id = state.info().user_id();
            let now = Utc::now().timestamp_millis();

            if !valid_finance_id(&input.quote_id)
                || !valid_finance_id(&input.pricing_version)
                || !valid_finance_id(&input.idempotency_key)
                || !valid_finance_id(&input.settlement_authority)
                || !valid_finance_id(&input.meter_program_id)
            {
                return Err(anyhow!(
                    "invalid quote, pricing, meter, authority, or idempotency identifier"
                ));
            }
            if !valid_finance_hash(&input.context_hash)
                || !valid_finance_hash(&input.beneficiary_plan_hash)
            {
                return Err(anyhow!(
                    "contextHash and beneficiaryPlanHash must be sha256 hex"
                ));
            }
            if input.max_amount <= 0 {
                return Err(anyhow!("maxAmount must be greater than zero"));
            }
            if input.expires_at <= now
                || input.expires_at
                    > now
                        .checked_add(FINANCE_HOLD_MAX_TTL_MS)
                        .ok_or_else(|| anyhow!("hold expiry overflow"))?
            {
                return Err(anyhow!(
                    "expiresAt must be in the future and within 24 hours"
                ));
            }
            if input.beneficiaries.is_empty()
                || input.beneficiaries.len() > FINANCE_MAX_BENEFICIARIES
            {
                return Err(anyhow!(
                    "beneficiaries must contain between 1 and 64 entries"
                ));
            }

            // A hold is not an arbitrary client-authored transfer plan. Load the
            // immutable quote and require the signed request to equal the exact
            // holdRequest the pricing creature persisted.
            let quote = trx
                .get_json(&format!("Json::BillingQuote::{}", input.quote_id), "quote")
                .map_err(|_| anyhow!("billing quote not found"))?;
            if quote.get("payerUserId").and_then(Value::as_str) != Some(payer_id.as_str()) {
                return Err(anyhow!("billing quote payer mismatch"));
            }
            let quote_expires_at = quote.get("expiresAt").and_then(as_i64).unwrap_or(0);
            if quote_expires_at <= 0 || now > quote_expires_at {
                return Err(anyhow!("billing quote expired"));
            }
            let signed_request = serde_json::to_value(&input)?;
            if quote.get("holdRequest") != Some(&signed_request) {
                return Err(anyhow!("hold request does not match server quote"));
            }
            let project_id = quote
                .get("projectId")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if !project_id.is_empty()
                && !security_app
                    .tools()
                    .security()
                    .has_access_to_store(&payer_id, &project_id)
            {
                return Err(anyhow!("payer is not a project member"));
            }
            if !trx.has_obj("Creature", &input.settlement_authority) {
                return Err(anyhow!("settlement authority not found"));
            }
            if !trx.has_obj("Program", &input.meter_program_id) {
                return Err(anyhow!("meter program not found"));
            }

            let computed_plan_hash = finance_beneficiary_plan_hash(&input.beneficiaries);
            if computed_plan_hash != input.beneficiary_plan_hash.to_ascii_lowercase() {
                return Err(anyhow!("beneficiary plan hash mismatch"));
            }
            let request_hash = finance_hash(&serde_json::to_value(&input)?)?;
            let request_marker =
                format!("FinanceHoldRequest::{payer_id}::{}", input.idempotency_key);
            let previous = trx.get_link(&request_marker);
            if !previous.is_empty() {
                let Some((hold_id, previous_hash)) = previous.split_once('|') else {
                    return Err(anyhow!("invalid hold idempotency record"));
                };
                if previous_hash != request_hash {
                    return Err(anyhow!(
                        "idempotency key already used with different request"
                    ));
                }
                let hold = get_finance_hold(&*trx, hold_id)?;
                return Ok(json!({
                    "applied": false,
                    "alreadyApplied": true,
                    "hold": hold,
                }));
            }

            let mut cap_total = 0_i64;
            let mut caps: HashMap<String, i64> = HashMap::new();
            let mut participants = vec![payer_id.clone(), input.settlement_authority.clone()];
            for beneficiary in &input.beneficiaries {
                if !valid_finance_id(&beneficiary.user_id)
                    || !valid_finance_id(&beneficiary.role)
                    || beneficiary.max_amount <= 0
                {
                    return Err(anyhow!("invalid beneficiary"));
                }
                if beneficiary.user_id == payer_id {
                    return Err(anyhow!("payer cannot be a hold beneficiary"));
                }
                let cap_key = format!("{}|{}", beneficiary.user_id, beneficiary.role);
                if caps.insert(cap_key, beneficiary.max_amount).is_some() {
                    return Err(anyhow!("duplicate beneficiary role"));
                }
                if !trx.has_obj("Creature", &beneficiary.user_id) {
                    return Err(anyhow!("beneficiary not found"));
                }
                cap_total = cap_total
                    .checked_add(beneficiary.max_amount)
                    .ok_or_else(|| anyhow!("beneficiary cap overflow"))?;
                participants.push(beneficiary.user_id.clone());
            }
            if cap_total != input.max_amount {
                return Err(anyhow!("beneficiary caps must equal maxAmount"));
            }

            let mut payer = Creature {
                id: payer_id.clone(),
                ..Default::default()
            }
            .pull(&*trx);
            if payer.id.is_empty() {
                return Err(anyhow!("payer creature not found"));
            }
            if finance_debt_amount(&*trx, &payer_id)? > 0 {
                return Err(anyhow!("wallet has outstanding payment debt"));
            }
            let withdrawable = finance_withdrawable_amount(&*trx, &payer_id)?;
            if withdrawable > payer.balance {
                return Err(anyhow!("withdrawable balance exceeds available balance"));
            }
            let nonwithdrawable = payer.balance - withdrawable;
            let withdrawable_amount = input.max_amount.saturating_sub(nonwithdrawable);
            payer.balance = payer
                .balance
                .checked_sub(input.max_amount)
                .ok_or_else(|| anyhow!("your balance is not enough"))?;
            set_finance_withdrawable_amount(
                &*trx,
                &payer_id,
                withdrawable
                    .checked_sub(withdrawable_amount)
                    .ok_or_else(|| anyhow!("withdrawable balance underflow"))?,
            )?;
            let held = finance_held_amount(&*trx, &payer_id)?
                .checked_add(input.max_amount)
                .ok_or_else(|| anyhow!("held balance overflow"))?;
            reserve_project_budget(&*trx, &project_id, input.max_amount, now)?;

            let hold_id = secure_unique_string();
            let hold = json!({
                "version": 2,
                "holdId": hold_id,
                "payerUserId": payer_id,
                "quoteId": input.quote_id,
                "pricingVersion": input.pricing_version,
                "maxAmount": input.max_amount,
                "remainingAmount": input.max_amount,
                "withdrawableAmount": withdrawable_amount,
                "meterProgramId": input.meter_program_id,
                "settlementAuthority": input.settlement_authority,
                "expiresAt": input.expires_at,
                "projectId": project_id,
                "contextHash": input.context_hash,
                "beneficiaryPlanHash": input.beneficiary_plan_hash.to_ascii_lowercase(),
                "beneficiaries": input.beneficiaries,
                "requestHash": request_hash,
                "status": "open",
                "createdAt": now,
            });
            let hold_map = hold
                .as_object()
                .cloned()
                .ok_or_else(|| anyhow!("invalid hold record"))?;

            payer.push(&*trx);
            set_finance_held_amount(&*trx, &payer_id, held)?;
            put_finance_hold(&*trx, &hold_id, &hold_map)?;
            trx.put_link(&request_marker, &format!("{hold_id}|{request_hash}"));
            trx.put_link(
                &format!("FinanceHoldByPayer::{payer_id}::{now:020}::{hold_id}"),
                &hold_id,
            );
            let journal_id = write_finance_journal(
                &*trx,
                "hold.created",
                &hold_id,
                &payer_id,
                json!({
                    "entries": [
                        {"account": format!("wallet:{payer_id}:available"), "amount": -input.max_amount},
                        {"account": format!("wallet:{payer_id}:held"), "amount": input.max_amount}
                    ],
                    "quoteId": input.quote_id,
                    "pricingVersion": input.pricing_version,
                    "projectId": project_id,
                }),
                &participants,
                now,
            )?;

            Ok(json!({
                "applied": true,
                "hold": hold_map,
                "journalId": journal_id,
            }))
        },
    )
}

fn start_hold(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<StartHoldInput, _>(
        app,
        "/creatures/startHold",
        finance_guard(),
        move |state: Arc<dyn IState>, input: StartHoldInput| -> Result<Value> {
            let trx = state.trx();
            let authority_id = state.info().user_id();
            let now = Utc::now().timestamp_millis();
            if !valid_finance_id(&input.hold_id)
                || !valid_finance_id(&input.payer_user_id)
                || !valid_finance_id(&input.quote_id)
                || !valid_finance_id(&input.run_id)
            {
                return Err(anyhow!("invalid run authorization"));
            }

            let run_marker = format!("FinanceRun::{authority_id}::{}", input.run_id);
            let previous_hold_id = trx.get_link(&run_marker);
            if !previous_hold_id.is_empty() {
                if previous_hold_id != input.hold_id {
                    return Err(anyhow!("run id already used for another hold"));
                }
                let hold = get_finance_hold(&*trx, &input.hold_id)?;
                return Ok(json!({
                    "applied": false,
                    "alreadyApplied": true,
                    "hold": hold,
                }));
            }

            let mut hold = get_finance_hold(&*trx, &input.hold_id)?;
            if hold.get("status").and_then(Value::as_str) != Some("open") {
                return Err(anyhow!("hold is not open"));
            }
            if hold.get("payerUserId").and_then(Value::as_str) != Some(input.payer_user_id.as_str())
            {
                return Err(anyhow!("payer does not match hold"));
            }
            if hold.get("quoteId").and_then(Value::as_str) != Some(input.quote_id.as_str()) {
                return Err(anyhow!("quote does not match hold"));
            }
            if hold.get("settlementAuthority").and_then(Value::as_str)
                != Some(authority_id.as_str())
            {
                return Err(anyhow!("caller is not the settlement authority"));
            }
            let expires_at = hold.get("expiresAt").and_then(as_i64).unwrap_or(0);
            if expires_at <= 0 || now > expires_at {
                return Err(anyhow!("hold expired"));
            }

            hold.insert("status".to_string(), json!("running"));
            hold.insert("runId".to_string(), json!(input.run_id));
            hold.insert("startedAt".to_string(), json!(now));
            put_finance_hold(&*trx, &input.hold_id, &hold)?;
            trx.put_link(&run_marker, &input.hold_id);
            let participants = vec![input.payer_user_id.clone(), authority_id];
            let journal_id = write_finance_journal(
                &*trx,
                "hold.started",
                &input.hold_id,
                &input.payer_user_id,
                json!({
                    "entries": [],
                    "quoteId": input.quote_id,
                    "runId": input.run_id,
                }),
                &participants,
                now,
            )?;
            Ok(json!({
                "applied": true,
                "hold": hold,
                "journalId": journal_id,
            }))
        },
    )
}

fn settle_hold(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<SettleHoldInput, _>(
        app,
        "/creatures/settleHold",
        finance_guard(),
        move |state: Arc<dyn IState>, input: SettleHoldInput| -> Result<Value> {
            let trx = state.trx();
            let authority_id = state.info().user_id();
            let now = Utc::now().timestamp_millis();

            if !valid_finance_id(&input.hold_id)
                || !valid_finance_id(&input.payer_user_id)
                || !valid_finance_id(&input.quote_id)
                || !valid_finance_id(&input.settlement_id)
                || !valid_finance_hash(&input.usage_hash)
            {
                return Err(anyhow!("invalid settlement identifiers or usageHash"));
            }
            let settlement_marker =
                format!("FinanceSettlement::{authority_id}::{}", input.settlement_id);
            let previous_hold_id = trx.get_link(&settlement_marker);
            if !previous_hold_id.is_empty() {
                if previous_hold_id != input.hold_id {
                    return Err(anyhow!("settlement id already used for another hold"));
                }
                let hold = get_finance_hold(&*trx, &input.hold_id)?;
                return Ok(json!({
                    "applied": false,
                    "alreadyApplied": true,
                    "hold": hold,
                }));
            }

            let mut hold = get_finance_hold(&*trx, &input.hold_id)?;
            if hold.get("status").and_then(Value::as_str) != Some("running") {
                return Err(anyhow!("hold is not running"));
            }
            if hold.get("payerUserId").and_then(Value::as_str) != Some(input.payer_user_id.as_str())
            {
                return Err(anyhow!("payer does not match hold"));
            }
            if hold.get("quoteId").and_then(Value::as_str) != Some(input.quote_id.as_str()) {
                return Err(anyhow!("quote does not match hold"));
            }
            if hold.get("runId").and_then(Value::as_str) != Some(input.settlement_id.as_str()) {
                return Err(anyhow!("settlement does not match authorized run"));
            }
            if hold.get("settlementAuthority").and_then(Value::as_str)
                != Some(authority_id.as_str())
            {
                return Err(anyhow!("caller is not the settlement authority"));
            }
            let expires_at = hold.get("expiresAt").and_then(as_i64).unwrap_or(0);
            if expires_at <= 0 || now > expires_at {
                return Err(anyhow!("hold expired"));
            }
            let max_amount = hold.get("maxAmount").and_then(as_i64).unwrap_or(0);
            if max_amount <= 0 {
                return Err(anyhow!("invalid hold amount"));
            }

            let beneficiaries = hold
                .get("beneficiaries")
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow!("hold beneficiaries missing"))?;
            let mut caps: HashMap<String, i64> = HashMap::new();
            for item in beneficiaries {
                let user_id = item.get("userId").and_then(Value::as_str).unwrap_or("");
                let role = item.get("role").and_then(Value::as_str).unwrap_or("");
                let cap = item.get("maxAmount").and_then(as_i64).unwrap_or(0);
                if user_id.is_empty() || role.is_empty() || cap <= 0 {
                    return Err(anyhow!("invalid hold beneficiary"));
                }
                caps.insert(format!("{user_id}|{role}"), cap);
            }

            let mut actual_amount = 0_i64;
            let mut allocated: HashMap<String, i64> = HashMap::new();
            let mut credits: HashMap<String, i64> = HashMap::new();
            for line in &input.lines {
                if line.amount <= 0
                    || !valid_finance_id(&line.user_id)
                    || !valid_finance_id(&line.role)
                    || line.source_ref.len() > 256
                {
                    return Err(anyhow!("invalid settlement line"));
                }
                let cap_key = format!("{}|{}", line.user_id, line.role);
                let Some(cap) = caps.get(&cap_key) else {
                    return Err(anyhow!(
                        "settlement beneficiary role not authorized by hold"
                    ));
                };
                actual_amount = actual_amount
                    .checked_add(line.amount)
                    .ok_or_else(|| anyhow!("settlement amount overflow"))?;
                let role_total = allocated.entry(cap_key).or_insert(0);
                *role_total = role_total
                    .checked_add(line.amount)
                    .ok_or_else(|| anyhow!("beneficiary role amount overflow"))?;
                if *role_total > *cap {
                    return Err(anyhow!("settlement exceeds beneficiary role cap"));
                }
                let credited = credits.entry(line.user_id.clone()).or_insert(0);
                *credited = credited
                    .checked_add(line.amount)
                    .ok_or_else(|| anyhow!("beneficiary amount overflow"))?;
            }
            if actual_amount > max_amount {
                return Err(anyhow!("settlement exceeds hold"));
            }
            let refund_amount = max_amount
                .checked_sub(actual_amount)
                .ok_or_else(|| anyhow!("refund underflow"))?;
            let project_id = hold
                .get("projectId")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            finalize_project_budget(&*trx, &project_id, max_amount, actual_amount, now)?;

            add_finance_counter(
                &*trx,
                &format!("FinanceSpent::{}", input.payer_user_id),
                actual_amount,
            )?;
            let mut participants = vec![input.payer_user_id.clone(), authority_id.clone()];
            let mut wallet_credits: HashMap<String, i64> = HashMap::new();
            let mut debt_repays: HashMap<String, i64> = HashMap::new();
            for (user_id, amount) in &credits {
                add_finance_counter(&*trx, &format!("FinanceEarned::{user_id}"), *amount)?;
                let mut receiver = Creature {
                    id: user_id.clone(),
                    ..Default::default()
                }
                .pull(&*trx);
                if receiver.id.is_empty() {
                    return Err(anyhow!("settlement beneficiary not found"));
                }
                let debt = finance_debt_amount(&*trx, user_id)?;
                let debt_repaid = debt.min(*amount);
                let wallet_credit = amount
                    .checked_sub(debt_repaid)
                    .ok_or_else(|| anyhow!("beneficiary credit underflow"))?;
                receiver.balance = receiver
                    .balance
                    .checked_add(wallet_credit)
                    .ok_or_else(|| anyhow!("beneficiary balance overflow"))?;
                let withdrawable = finance_withdrawable_amount(&*trx, user_id)?
                    .checked_add(wallet_credit)
                    .ok_or_else(|| anyhow!("withdrawable earnings overflow"))?;
                set_finance_debt_amount(&*trx, user_id, debt - debt_repaid)?;
                set_finance_withdrawable_amount(&*trx, user_id, withdrawable)?;
                wallet_credits.insert(user_id.clone(), wallet_credit);
                debt_repays.insert(user_id.clone(), debt_repaid);
                receiver.push(&*trx);
                participants.push(user_id.clone());
            }

            let mut payer = Creature {
                id: input.payer_user_id.clone(),
                ..Default::default()
            }
            .pull(&*trx);
            if payer.id.is_empty() {
                return Err(anyhow!("payer creature not found"));
            }
            payer.balance = payer
                .balance
                .checked_add(refund_amount)
                .ok_or_else(|| anyhow!("payer balance overflow"))?;
            let held_withdrawable = hold
                .get("withdrawableAmount")
                .and_then(as_i64)
                .unwrap_or(0);
            let withdrawable_refund = refund_amount.min(held_withdrawable);
            let withdrawable_spent = held_withdrawable
                .checked_sub(withdrawable_refund)
                .ok_or_else(|| anyhow!("withdrawable settlement underflow"))?;
            let payer_withdrawable = finance_withdrawable_amount(&*trx, &input.payer_user_id)?
                .checked_add(withdrawable_refund)
                .ok_or_else(|| anyhow!("withdrawable refund overflow"))?;
            set_finance_withdrawable_amount(&*trx, &input.payer_user_id, payer_withdrawable)?;
            payer.push(&*trx);
            let held = finance_held_amount(&*trx, &input.payer_user_id)?
                .checked_sub(max_amount)
                .ok_or_else(|| anyhow!("held balance underflow"))?;
            set_finance_held_amount(&*trx, &input.payer_user_id, held)?;

            hold.insert("status".to_string(), json!("settled"));
            hold.insert("remainingAmount".to_string(), json!(0));
            hold.insert("actualAmount".to_string(), json!(actual_amount));
            hold.insert("refundedAmount".to_string(), json!(refund_amount));
            hold.insert("withdrawableRefundedAmount".to_string(), json!(withdrawable_refund));
            hold.insert("withdrawableSpentAmount".to_string(), json!(withdrawable_spent));
            hold.insert("settlementId".to_string(), json!(input.settlement_id));
            hold.insert("usageHash".to_string(), json!(input.usage_hash));
            hold.insert(
                "settlementLines".to_string(),
                serde_json::to_value(&input.lines)?,
            );
            hold.insert("finalizedAt".to_string(), json!(now));
            put_finance_hold(&*trx, &input.hold_id, &hold)?;
            trx.put_link(&settlement_marker, &input.hold_id);

            let mut entries = vec![
                json!({
                    "account": format!("wallet:{}:held", input.payer_user_id),
                    "amount": -max_amount,
                }),
                json!({
                    "account": format!("wallet:{}:available", input.payer_user_id),
                    "amount": refund_amount,
                }),
            ];
            for (user_id, gross_amount) in &credits {
                let wallet_credit = wallet_credits.get(user_id).copied().unwrap_or(0);
                let debt_repaid = debt_repays.get(user_id).copied().unwrap_or(0);
                entries.push(json!({
                    "account": format!("wallet:{user_id}:available"),
                    "amount": wallet_credit,
                    "grossAmount": gross_amount,
                }));
                if debt_repaid > 0 {
                    entries.push(json!({
                        "account": format!("wallet:{user_id}:debt"),
                        "amount": -debt_repaid,
                    }));
                }
            }
            let journal_id = write_finance_journal(
                &*trx,
                "hold.settled",
                &input.hold_id,
                &input.payer_user_id,
                json!({
                    "entries": entries,
                    "quoteId": input.quote_id,
                    "settlementId": input.settlement_id,
                    "usageHash": input.usage_hash,
                    "actualAmount": actual_amount,
                    "refundedAmount": refund_amount,
                    "settlementLines": input.lines,
                }),
                &participants,
                now,
            )?;

            Ok(json!({
                "applied": true,
                "hold": hold,
                "journalId": journal_id,
            }))
        },
    )
}

fn release_hold(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<ReleaseHoldInput, _>(
        app,
        "/creatures/releaseHold",
        finance_guard(),
        move |state: Arc<dyn IState>, input: ReleaseHoldInput| -> Result<Value> {
            let trx = state.trx();
            let caller_id = state.info().user_id();
            let now = Utc::now().timestamp_millis();

            if !valid_finance_id(&input.hold_id)
                || !valid_finance_id(&input.payer_user_id)
                || !valid_finance_id(&input.release_id)
                || input.reason.len() > 256
            {
                return Err(anyhow!("invalid release request"));
            }
            let release_marker = format!("FinanceRelease::{caller_id}::{}", input.release_id);
            let previous_hold_id = trx.get_link(&release_marker);
            if !previous_hold_id.is_empty() {
                if previous_hold_id != input.hold_id {
                    return Err(anyhow!("release id already used for another hold"));
                }
                let hold = get_finance_hold(&*trx, &input.hold_id)?;
                return Ok(json!({
                    "applied": false,
                    "alreadyApplied": true,
                    "hold": hold,
                }));
            }

            let mut hold = get_finance_hold(&*trx, &input.hold_id)?;
            let active_status = hold.get("status").and_then(Value::as_str).unwrap_or("");
            if active_status != "open" && active_status != "running" {
                return Err(anyhow!("hold is not active"));
            }
            if hold.get("payerUserId").and_then(Value::as_str) != Some(input.payer_user_id.as_str())
            {
                return Err(anyhow!("payer does not match hold"));
            }
            let authority = hold
                .get("settlementAuthority")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let expires_at = hold.get("expiresAt").and_then(as_i64).unwrap_or(0);
            let payer_open_release = caller_id == input.payer_user_id && active_status == "open";
            let payer_expired_release = caller_id == input.payer_user_id && now >= expires_at;
            if caller_id != authority && !payer_open_release && !payer_expired_release {
                return Err(anyhow!("only the authority may release an active hold"));
            }
            let max_amount = hold.get("maxAmount").and_then(as_i64).unwrap_or(0);
            if max_amount <= 0 {
                return Err(anyhow!("invalid hold amount"));
            }
            let project_id = hold
                .get("projectId")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            finalize_project_budget(&*trx, &project_id, max_amount, 0, now)?;

            let mut payer = Creature {
                id: input.payer_user_id.clone(),
                ..Default::default()
            }
            .pull(&*trx);
            if payer.id.is_empty() {
                return Err(anyhow!("payer creature not found"));
            }
            payer.balance = payer
                .balance
                .checked_add(max_amount)
                .ok_or_else(|| anyhow!("payer balance overflow"))?;
            let withdrawable_refund = hold
                .get("withdrawableAmount")
                .and_then(as_i64)
                .unwrap_or(0);
            let withdrawable = finance_withdrawable_amount(&*trx, &input.payer_user_id)?
                .checked_add(withdrawable_refund)
                .ok_or_else(|| anyhow!("withdrawable refund overflow"))?;
            set_finance_withdrawable_amount(&*trx, &input.payer_user_id, withdrawable)?;
            payer.push(&*trx);
            let held = finance_held_amount(&*trx, &input.payer_user_id)?
                .checked_sub(max_amount)
                .ok_or_else(|| anyhow!("held balance underflow"))?;
            set_finance_held_amount(&*trx, &input.payer_user_id, held)?;

            let status = if now >= expires_at {
                "expired"
            } else {
                "released"
            };
            hold.insert("status".to_string(), json!(status));
            hold.insert("remainingAmount".to_string(), json!(0));
            hold.insert("refundedAmount".to_string(), json!(max_amount));
            hold.insert("withdrawableRefundedAmount".to_string(), json!(withdrawable_refund));
            hold.insert("releaseId".to_string(), json!(input.release_id));
            hold.insert("releaseReason".to_string(), json!(input.reason));
            hold.insert("finalizedAt".to_string(), json!(now));
            put_finance_hold(&*trx, &input.hold_id, &hold)?;
            trx.put_link(&release_marker, &input.hold_id);

            let participants = vec![input.payer_user_id.clone(), authority];
            let journal_id = write_finance_journal(
                &*trx,
                "hold.released",
                &input.hold_id,
                &input.payer_user_id,
                json!({
                    "entries": [
                        {
                            "account": format!("wallet:{}:held", input.payer_user_id),
                            "amount": -max_amount,
                        },
                        {
                            "account": format!("wallet:{}:available", input.payer_user_id),
                            "amount": max_amount,
                        }
                    ],
                    "status": status,
                    "reason": input.reason,
                }),
                &participants,
                now,
            )?;

            Ok(json!({
                "applied": true,
                "hold": hold,
                "journalId": journal_id,
            }))
        },
    )
}

fn get_hold(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<GetHoldInput, _>(
        app,
        "/creatures/getHold",
        finance_guard(),
        move |state: Arc<dyn IState>, input: GetHoldInput| -> Result<Value> {
            if !valid_finance_id(&input.hold_id) {
                return Err(anyhow!("invalid hold id"));
            }
            let hold = get_finance_hold(&*state.trx(), &input.hold_id)?;
            let payer_id = hold
                .get("payerUserId")
                .and_then(Value::as_str)
                .unwrap_or("");
            if !input.payer_user_id.is_empty() && input.payer_user_id != payer_id {
                return Err(anyhow!("payer does not match hold"));
            }
            let caller_id = state.info().user_id();
            let authority = hold
                .get("settlementAuthority")
                .and_then(Value::as_str)
                .unwrap_or("");
            let is_beneficiary = hold
                .get("beneficiaries")
                .and_then(Value::as_array)
                .map(|items| {
                    items.iter().any(|item| {
                        item.get("userId").and_then(Value::as_str) == Some(caller_id.as_str())
                    })
                })
                .unwrap_or(false);
            if caller_id != payer_id && caller_id != authority && !is_beneficiary {
                return Err(anyhow!("access denied"));
            }
            Ok(json!({"hold": hold}))
        },
    )
}

fn finance_payout_key(payout_id: &str) -> String {
    format!("Json::FinancePayout::{payout_id}")
}

fn get_finance_payout(trx: &dyn ITrx, payout_id: &str) -> Result<Map<String, Value>> {
    trx.get_json(&finance_payout_key(payout_id), "payout")
        .map_err(|_| anyhow!("payout not found"))
}

fn finance_payout_records(trx: &dyn ITrx, user_id: &str, limit: usize) -> Vec<Value> {
    let mut keys = trx
        .get_links_list(&format!("FinancePayoutByUser::{user_id}::"), -1, -1, &[])
        .unwrap_or_default();
    keys.sort();
    keys.reverse();
    let mut payouts = Vec::new();
    for key in keys.into_iter().take(limit) {
        let payout_id = trx.get_link(&key);
        if let Ok(payout) = get_finance_payout(trx, &payout_id) {
            payouts.push(Value::Object(payout));
        }
    }
    payouts
}

fn financial_account_snapshot(trx: &dyn ITrx, user_id: &str, limit: usize) -> Result<Value> {
    let creature = Creature {
        id: user_id.to_string(),
        ..Default::default()
    }
    .pull(trx);
    if creature.id.is_empty() {
        return Err(anyhow!("financial account not found"));
    }
    let mut journal_keys = trx
        .get_links_list(
            &format!("FinanceJournalByUser::{user_id}::"),
            -1,
            -1,
            &[],
        )
        .unwrap_or_default();
    journal_keys.sort();
    journal_keys.reverse();
    let mut transactions = Vec::new();
    for key in journal_keys.into_iter().take(limit) {
        let journal_id = trx.get_link(&key);
        if !journal_id.is_empty() {
            if let Ok(entry) = trx.get_json(&format!("Json::FinanceJournal::{journal_id}"), "entry")
            {
                transactions.push(Value::Object(entry));
            }
        }
    }
    let mut hold_keys = trx
        .get_links_list(
            &format!("FinanceHoldByPayer::{user_id}::"),
            -1,
            -1,
            &[],
        )
        .unwrap_or_default();
    hold_keys.sort();
    hold_keys.reverse();
    let mut active_holds = Vec::new();
    for key in hold_keys.into_iter().take(100) {
        let hold_id = trx.get_link(&key);
        if let Ok(hold) = get_finance_hold(trx, &hold_id) {
            let status = hold.get("status").and_then(Value::as_str).unwrap_or("");
            if status == "open" || status == "running" {
                active_holds.push(Value::Object(hold));
            }
        }
    }
    Ok(json!({
        "userId": user_id,
        "availableMinor": creature.balance,
        "heldMinor": finance_held_amount(trx, user_id)?,
        "debtMinor": finance_debt_amount(trx, user_id)?,
        "withdrawableMinor": finance_withdrawable_amount(trx, user_id)?,
        "payoutHeldMinor": finance_payout_held_amount(trx, user_id)?,
        "earnedMinor": finance_counter(trx, &format!("FinanceEarned::{user_id}"))?,
        "spentMinor": finance_counter(trx, &format!("FinanceSpent::{user_id}"))?,
        "activeHolds": active_holds,
        "transactions": transactions,
        "payouts": finance_payout_records(trx, user_id, limit),
    }))
}

fn get_financial_account(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<GetFinancialAccountInput, _>(
        app,
        "/creatures/getFinancialAccount",
        finance_guard(),
        move |state: Arc<dyn IState>, input: GetFinancialAccountInput| -> Result<Value> {
            let caller_id = state.info().user_id();
            let user_id = if input.user_id.is_empty() {
                caller_id.clone()
            } else {
                input.user_id
            };
            if !valid_finance_id(&user_id) {
                return Err(anyhow!("invalid financial account id"));
            }
            if user_id != caller_id && caller_id != "1@global" {
                return Err(anyhow!("access denied"));
            }
            let limit = if input.limit <= 0 {
                50
            } else {
                input.limit.min(100) as usize
            };
            financial_account_snapshot(&*state.trx(), &user_id, limit)
        },
    )
}

fn finance_map_add(totals: &mut HashMap<String, i64>, key: &str, amount: i64) -> bool {
    if key.is_empty() || amount < 0 {
        return false;
    }
    let current = totals.get(key).copied().unwrap_or(0);
    let Some(next) = current.checked_add(amount) else {
        return false;
    };
    totals.insert(key.to_string(), next);
    true
}

fn request_payout(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<RequestPayoutInput, _>(
        app,
        "/creatures/requestPayout",
        finance_guard(),
        move |state: Arc<dyn IState>, input: RequestPayoutInput| -> Result<Value> {
            let user_id = state.info().user_id();
            let destination = input.destination_ref.trim();
            if !valid_finance_id(&input.request_id)
                || input.amount <= 0
                || destination.is_empty()
                || destination.len() > 256
                || destination.chars().any(char::is_control)
            {
                return Err(anyhow!("invalid payout request"));
            }
            let trx = state.trx();
            let request_hash = finance_hash(&serde_json::to_value(&input)?)?;
            let marker = format!("FinancePayoutRequest::{user_id}::{}", input.request_id);
            let previous = trx.get_link(&marker);
            if !previous.is_empty() {
                let Some((payout_id, previous_hash)) = previous.split_once(char::from(124)) else {
                    return Err(anyhow!("invalid payout idempotency record"));
                };
                if previous_hash != request_hash {
                    return Err(anyhow!("requestId already used with different payout data"));
                }
                return Ok(json!({
                    "applied": false,
                    "alreadyApplied": true,
                    "payout": get_finance_payout(&*trx, payout_id)?,
                }));
            }
            if finance_debt_amount(&*trx, &user_id)? > 0 {
                return Err(anyhow!("wallet has outstanding payment debt"));
            }
            let mut creature = Creature { id: user_id.clone(), ..Default::default() }.pull(&*trx);
            if creature.id.is_empty() {
                return Err(anyhow!("financial account not found"));
            }
            let withdrawable = finance_withdrawable_amount(&*trx, &user_id)?;
            if input.amount > withdrawable || input.amount > creature.balance {
                return Err(anyhow!("withdrawable earnings are not enough"));
            }
            creature.balance = creature
                .balance
                .checked_sub(input.amount)
                .ok_or_else(|| anyhow!("wallet payout underflow"))?;
            let next_withdrawable = withdrawable
                .checked_sub(input.amount)
                .ok_or_else(|| anyhow!("withdrawable payout underflow"))?;
            let payout_held = finance_payout_held_amount(&*trx, &user_id)?
                .checked_add(input.amount)
                .ok_or_else(|| anyhow!("payout held overflow"))?;
            let now = Utc::now().timestamp_millis();
            let payout_id = secure_unique_string();
            let payout = json!({
                "payoutId": payout_id,
                "requestId": input.request_id,
                "userId": user_id,
                "amount": input.amount,
                "destinationRef": destination,
                "status": "pending",
                "createdAt": now,
                "requestHash": request_hash,
            });
            let payout_map = payout.as_object().cloned().ok_or_else(|| anyhow!("invalid payout record"))?;
            creature.push(&*trx);
            set_finance_withdrawable_amount(&*trx, &user_id, next_withdrawable)?;
            set_finance_payout_held_amount(&*trx, &user_id, payout_held)?;
            trx.put_json(&finance_payout_key(&payout_id), "payout", &payout, false)?;
            trx.put_link(&marker, &format!("{}{}{}", payout_id, char::from(124), request_hash));
            trx.put_link(
                &format!("FinancePayoutByUser::{user_id}::{now:020}::{payout_id}"),
                &payout_id,
            );
            let journal_id = write_finance_journal(
                &*trx,
                "payout.requested",
                "",
                &user_id,
                json!({
                    "entries": [
                        {"account": format!("wallet:{user_id}:available"), "amount": -input.amount},
                        {"account": format!("wallet:{user_id}:payout_held"), "amount": input.amount}
                    ],
                    "payoutId": payout_id,
                    "destinationRef": destination,
                }),
                std::slice::from_ref(&user_id),
                now,
            )?;
            Ok(json!({"applied": true, "payout": payout_map, "journalId": journal_id}))
        },
    )
}

fn resolve_payout(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<ResolvePayoutInput, _>(
        app,
        "/creatures/resolvePayout",
        finance_guard(),
        move |state: Arc<dyn IState>, input: ResolvePayoutInput| -> Result<Value> {
            if state.info().user_id() != "1@global" {
                return Err(anyhow!("access denied"));
            }
            if !valid_finance_id(&input.payout_id)
                || !valid_finance_id(&input.resolution_id)
                || (input.status != "paid" && input.status != "rejected")
                || input.provider_reference.len() > 256
                || input.provider_reference.chars().any(char::is_control)
                || input.reason.len() > 256
                || input.reason.chars().any(char::is_control)
                || (input.status == "paid" && input.provider_reference.trim().is_empty())
            {
                return Err(anyhow!("invalid payout resolution"));
            }
            let trx = state.trx();
            let request_hash = finance_hash(&serde_json::to_value(&input)?)?;
            let marker = format!("FinancePayoutResolution::{}", input.resolution_id);
            let previous = trx.get_link(&marker);
            if !previous.is_empty() {
                let Some((payout_id, previous_hash)) = previous.split_once(char::from(124)) else {
                    return Err(anyhow!("invalid payout resolution idempotency record"));
                };
                if payout_id != input.payout_id || previous_hash != request_hash {
                    return Err(anyhow!("resolutionId already used with different payout data"));
                }
                return Ok(json!({
                    "applied": false,
                    "alreadyApplied": true,
                    "payout": get_finance_payout(&*trx, payout_id)?,
                }));
            }
            let mut payout = get_finance_payout(&*trx, &input.payout_id)?;
            if payout.get("status").and_then(Value::as_str) != Some("pending") {
                return Err(anyhow!("payout is not pending"));
            }
            let user_id = payout.get("userId").and_then(Value::as_str).unwrap_or("").to_string();
            let amount = payout.get("amount").and_then(as_i64).unwrap_or(0);
            if user_id.is_empty() || amount <= 0 {
                return Err(anyhow!("invalid payout record"));
            }
            let payout_held = finance_payout_held_amount(&*trx, &user_id)?
                .checked_sub(amount)
                .ok_or_else(|| anyhow!("payout held underflow"))?;
            set_finance_payout_held_amount(&*trx, &user_id, payout_held)?;
            let mut entries = vec![json!({
                "account": format!("wallet:{user_id}:payout_held"),
                "amount": -amount,
            })];
            if input.status == "rejected" {
                let mut creature = Creature { id: user_id.clone(), ..Default::default() }.pull(&*trx);
                if creature.id.is_empty() {
                    return Err(anyhow!("payout owner not found"));
                }
                creature.balance = creature.balance.checked_add(amount).ok_or_else(|| anyhow!("payout refund overflow"))?;
                let withdrawable = finance_withdrawable_amount(&*trx, &user_id)?
                    .checked_add(amount)
                    .ok_or_else(|| anyhow!("withdrawable payout refund overflow"))?;
                creature.push(&*trx);
                set_finance_withdrawable_amount(&*trx, &user_id, withdrawable)?;
                entries.push(json!({"account": format!("wallet:{user_id}:available"), "amount": amount}));
            } else {
                entries.push(json!({"account": "external:payouts", "amount": amount}));
            }
            let now = Utc::now().timestamp_millis();
            payout.insert("status".to_string(), json!(input.status));
            payout.insert("providerReference".to_string(), json!(input.provider_reference));
            payout.insert("reason".to_string(), json!(input.reason));
            payout.insert("resolutionId".to_string(), json!(input.resolution_id));
            payout.insert("resolvedAt".to_string(), json!(now));
            trx.put_json(
                &finance_payout_key(&input.payout_id),
                "payout",
                &Value::Object(payout.clone()),
                false,
            )?;
            trx.put_link(&marker, &format!("{}{}{}", input.payout_id, char::from(124), request_hash));
            let participants = vec![user_id.clone(), state.info().user_id()];
            let journal_id = write_finance_journal(
                &*trx,
                &format!("payout.{}", input.status),
                "",
                &user_id,
                json!({
                    "entries": entries,
                    "payoutId": input.payout_id,
                    "providerReference": input.provider_reference,
                    "reason": input.reason,
                }),
                &participants,
                now,
            )?;
            Ok(json!({"applied": true, "payout": payout, "journalId": journal_id}))
        },
    )
}

fn list_payouts(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<ListPayoutsInput, _>(
        app,
        "/creatures/listPayouts",
        finance_guard(),
        move |state: Arc<dyn IState>, input: ListPayoutsInput| -> Result<Value> {
            let caller = state.info().user_id();
            let limit = if input.limit <= 0 { 50_usize } else { input.limit.min(200) as usize };
            if input.user_id.is_empty() {
                if caller != "1@global" {
                    return Ok(json!({"payouts": finance_payout_records(&*state.trx(), &caller, limit)}));
                }
                let trx = state.trx();
                let prefix = "json::Json::FinancePayout::";
                let mut payouts: Vec<Value> = Vec::new();
                for key in trx.get_by_prefix(prefix) {
                    let Some(payout_id) = key.strip_prefix(prefix).and_then(|rest| rest.strip_suffix("::payout")) else { continue; };
                    if let Ok(payout) = get_finance_payout(&*trx, payout_id) {
                        payouts.push(Value::Object(payout));
                    }
                }
                payouts.sort_by(|a, b| {
                    b.get("createdAt").and_then(as_i64).unwrap_or(0)
                        .cmp(&a.get("createdAt").and_then(as_i64).unwrap_or(0))
                });
                payouts.truncate(limit);
                return Ok(json!({"payouts": payouts}));
            }
            if input.user_id != caller && caller != "1@global" {
                return Err(anyhow!("access denied"));
            }
            Ok(json!({
                "payouts": finance_payout_records(&*state.trx(), &input.user_id, limit),
            }))
        },
    )
}

fn reconcile_financial_system(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<ReconcileFinancialSystemInput, _>(
        app,
        "/creatures/reconcileFinancialSystem",
        finance_guard(),
        move |state: Arc<dyn IState>, input: ReconcileFinancialSystemInput| -> Result<Value> {
            if state.info().user_id() != "1@global" {
                return Err(anyhow!("access denied"));
            }
            let trx = state.trx();
            let max_issues = if input.max_issues <= 0 {
                100_usize
            } else {
                input.max_issues.min(1000) as usize
            };
            let mut issues: Vec<Value> = Vec::new();
            let mut report = |code: &str, reference: &str, detail: String| {
                if issues.len() < max_issues {
                    issues.push(json!({"code": code, "reference": reference, "detail": detail}));
                }
            };
            let mut held_expected: HashMap<String, i64> = HashMap::new();
            let mut project_reserved_expected: HashMap<String, i64> = HashMap::new();
            let mut project_spent_expected: HashMap<String, i64> = HashMap::new();
            let mut spent_expected: HashMap<String, i64> = HashMap::new();
            let mut earned_expected: HashMap<String, i64> = HashMap::new();
            let mut hold_count = 0_i64;
            let mut active_hold_count = 0_i64;
            let hold_prefix = "json::Json::FinanceHold::";
            for key in trx.get_by_prefix(hold_prefix) {
                let Some(hold_id) = key
                    .strip_prefix(hold_prefix)
                    .and_then(|rest| rest.strip_suffix("::hold"))
                else {
                    continue;
                };
                let Ok(hold) = trx.get_json(&finance_hold_key(hold_id), "hold") else {
                    report("hold.unreadable", hold_id, "hold JSON cannot be read".to_string());
                    continue;
                };
                hold_count += 1;
                let payer = hold.get("payerUserId").and_then(Value::as_str).unwrap_or("");
                let project = hold.get("projectId").and_then(Value::as_str).unwrap_or("");
                let status = hold.get("status").and_then(Value::as_str).unwrap_or("");
                let max_amount = hold.get("maxAmount").and_then(as_i64).unwrap_or(-1);
                let remaining = hold.get("remainingAmount").and_then(as_i64).unwrap_or(-1);
                if payer.is_empty() || max_amount <= 0 {
                    report("hold.invalid", hold_id, "payer or maxAmount is invalid".to_string());
                    continue;
                }
                match status {
                    "open" | "running" => {
                        active_hold_count += 1;
                        if remaining != max_amount {
                            report("hold.remaining_mismatch", hold_id, format!("remaining={remaining}, max={max_amount}"));
                        }
                        if !finance_map_add(&mut held_expected, payer, max_amount) {
                            report("held.overflow", payer, "expected held balance overflow".to_string());
                        }
                        if !project.is_empty() && !finance_map_add(&mut project_reserved_expected, project, max_amount) {
                            report("project.reserved_overflow", project, "expected reservation overflow".to_string());
                        }
                    }
                    "settled" => {
                        let actual = hold.get("actualAmount").and_then(as_i64).unwrap_or(-1);
                        let refunded = hold.get("refundedAmount").and_then(as_i64).unwrap_or(-1);
                        if remaining != 0 || actual < 0 || refunded < 0 || actual.checked_add(refunded) != Some(max_amount) {
                            report("hold.settlement_mismatch", hold_id, format!("actual={actual}, refunded={refunded}, max={max_amount}, remaining={remaining}"));
                            continue;
                        }
                        let mut line_total = 0_i64;
                        for line in hold.get("settlementLines").and_then(Value::as_array).cloned().unwrap_or_default() {
                            let user_id = line.get("userId").and_then(Value::as_str).unwrap_or("");
                            let amount = line.get("amount").and_then(as_i64).unwrap_or(-1);
                            if amount <= 0 || !finance_map_add(&mut earned_expected, user_id, amount) {
                                report("settlement.line_invalid", hold_id, "invalid beneficiary settlement line".to_string());
                                continue;
                            }
                            line_total = line_total.checked_add(amount).unwrap_or(i64::MAX);
                        }
                        if line_total != actual {
                            report("settlement.lines_mismatch", hold_id, format!("lines={line_total}, actual={actual}"));
                        }
                        if !finance_map_add(&mut spent_expected, payer, actual) {
                            report("spent.overflow", payer, "expected spent counter overflow".to_string());
                        }
                        if !project.is_empty() && !finance_map_add(&mut project_spent_expected, project, actual) {
                            report("project.spent_overflow", project, "expected project spend overflow".to_string());
                        }
                    }
                    "released" | "expired" => {
                        let refunded = hold.get("refundedAmount").and_then(as_i64).unwrap_or(-1);
                        if remaining != 0 || refunded != max_amount {
                            report("hold.release_mismatch", hold_id, format!("refunded={refunded}, max={max_amount}, remaining={remaining}"));
                        }
                    }
                    _ => report("hold.status_invalid", hold_id, format!("status={status}")),
                }
            }

            let mut held_actual: HashMap<String, i64> = HashMap::new();
            for key in trx.get_links_list("FinanceHeld::", -1, -1, &[]).unwrap_or_default() {
                let payer = key.strip_prefix("FinanceHeld::").unwrap_or("");
                let raw = trx.get_link(&key);
                match raw.parse::<i64>() {
                    Ok(value) if value >= 0 => { held_actual.insert(payer.to_string(), value); }
                    _ => report("held.invalid", payer, format!("stored={raw}")),
                }
            }
            let mut held_users: Vec<String> = held_expected.keys().chain(held_actual.keys()).cloned().collect();
            held_users.sort();
            held_users.dedup();
            for payer in held_users {
                let expected = held_expected.get(&payer).copied().unwrap_or(0);
                let actual = held_actual.get(&payer).copied().unwrap_or(0);
                if actual != expected {
                    report("held.mismatch", &payer, format!("stored={actual}, expected={expected}"));
                }
            }

            let mut payout_held_expected: HashMap<String, i64> = HashMap::new();
            let mut payout_count = 0_i64;
            let mut pending_payout_count = 0_i64;
            let payout_prefix = "json::Json::FinancePayout::";
            for key in trx.get_by_prefix(payout_prefix) {
                let Some(payout_id) = key
                    .strip_prefix(payout_prefix)
                    .and_then(|rest| rest.strip_suffix("::payout"))
                else {
                    continue;
                };
                let Ok(payout) = get_finance_payout(&*trx, payout_id) else {
                    report("payout.unreadable", payout_id, "payout JSON cannot be read".to_string());
                    continue;
                };
                payout_count += 1;
                if payout.get("status").and_then(Value::as_str) == Some("pending") {
                    pending_payout_count += 1;
                    let user_id = payout.get("userId").and_then(Value::as_str).unwrap_or("");
                    let amount = payout.get("amount").and_then(as_i64).unwrap_or(-1);
                    if amount <= 0 || !finance_map_add(&mut payout_held_expected, user_id, amount) {
                        report("payout.invalid", payout_id, "pending payout owner or amount is invalid".to_string());
                    }
                }
            }
            let mut payout_held_actual: HashMap<String, i64> = HashMap::new();
            for key in trx.get_links_list("FinancePayoutHeld::", -1, -1, &[]).unwrap_or_default() {
                let user_id = key.strip_prefix("FinancePayoutHeld::").unwrap_or("");
                match trx.get_link(&key).parse::<i64>() {
                    Ok(value) if value >= 0 => { payout_held_actual.insert(user_id.to_string(), value); }
                    _ => report("payout.held_invalid", user_id, "stored payout held amount is invalid".to_string()),
                }
            }
            let mut payout_users: Vec<String> = payout_held_expected
                .keys()
                .chain(payout_held_actual.keys())
                .cloned()
                .collect();
            payout_users.sort();
            payout_users.dedup();
            for user_id in payout_users {
                let expected = payout_held_expected.get(&user_id).copied().unwrap_or(0);
                let actual = payout_held_actual.get(&user_id).copied().unwrap_or(0);
                if actual != expected {
                    report("payout.held_mismatch", &user_id, format!("stored={actual}, expected={expected}"));
                }
            }
            for key in trx.get_links_list("FinanceWithdrawable::", -1, -1, &[]).unwrap_or_default() {
                let user_id = key.strip_prefix("FinanceWithdrawable::").unwrap_or("");
                let withdrawable = trx.get_link(&key).parse::<i64>().unwrap_or(-1);
                let creature = Creature { id: user_id.to_string(), ..Default::default() }.pull(&*trx);
                if withdrawable < 0 || creature.id.is_empty() || withdrawable > creature.balance {
                    report(
                        "withdrawable.invalid",
                        user_id,
                        format!("withdrawable={withdrawable}, available={}", creature.balance),
                    );
                }
            }

            for (prefix, expected, code) in [
                ("FinanceSpent::", &spent_expected, "spent.mismatch"),
                ("FinanceEarned::", &earned_expected, "earned.mismatch"),
            ] {
                let mut actual: HashMap<String, i64> = HashMap::new();
                for key in trx.get_links_list(prefix, -1, -1, &[]).unwrap_or_default() {
                    let user_id = key.strip_prefix(prefix).unwrap_or("");
                    if let Ok(value) = trx.get_link(&key).parse::<i64>() {
                        actual.insert(user_id.to_string(), value);
                    } else {
                        report(code, user_id, "stored counter is invalid".to_string());
                    }
                }
                let mut users: Vec<String> = expected.keys().chain(actual.keys()).cloned().collect();
                users.sort();
                users.dedup();
                for user_id in users {
                    let expected_value = expected.get(&user_id).copied().unwrap_or(0);
                    let actual_value = actual.get(&user_id).copied().unwrap_or(0);
                    if actual_value != expected_value {
                        report(code, &user_id, format!("stored={actual_value}, expected={expected_value}"));
                    }
                }
            }

            let mut projects: Vec<String> = project_reserved_expected
                .keys()
                .chain(project_spent_expected.keys())
                .cloned()
                .collect();
            let project_prefix = "json::Json::FinanceProjectBudget::";
            for key in trx.get_by_prefix(project_prefix) {
                if let Some(project) = key.strip_prefix(project_prefix).and_then(|rest| rest.strip_suffix("::budget")) {
                    projects.push(project.to_string());
                }
            }
            projects.sort();
            projects.dedup();
            for project in projects {
                let state = trx.get_json(&finance_project_budget_key(&project), "budget").unwrap_or_default();
                let stored_reserved = state.get("reservedMinor").and_then(as_i64).unwrap_or(0);
                let stored_spent = state.get("spentMinor").and_then(as_i64).unwrap_or(0);
                let expected_reserved = project_reserved_expected.get(&project).copied().unwrap_or(0);
                let expected_spent = project_spent_expected.get(&project).copied().unwrap_or(0);
                if stored_reserved != expected_reserved {
                    report("project.reserved_mismatch", &project, format!("stored={stored_reserved}, expected={expected_reserved}"));
                }
                if stored_spent < expected_spent {
                    report("project.spent_undercount", &project, format!("stored={stored_spent}, minimum={expected_spent}"));
                }
            }

            let issue_count = issues.len();
            Ok(json!({
                "healthy": issue_count == 0,
                "checkedAt": Utc::now().timestamp_millis(),
                "holdCount": hold_count,
                "activeHoldCount": active_hold_count,
                "payoutCount": payout_count,
                "pendingPayoutCount": pending_payout_count,
                "payerCount": held_expected.len(),
                "projectCount": project_reserved_expected.keys().chain(project_spent_expected.keys()).collect::<std::collections::HashSet<_>>().len(),
                "issueCount": issue_count,
                "issues": issues,
            }))
        },
    )
}

fn payment_adjustment(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<PaymentAdjustmentInput, _>(
        app,
        "/creatures/paymentAdjustment",
        finance_guard(),
        move |state: Arc<dyn IState>, input: PaymentAdjustmentInput| -> Result<Value> {
            if state.info().user_id() != "1@global" {
                return Err(anyhow!("access denied"));
            }
            if !valid_finance_id(&input.user_id)
                || !valid_finance_id(&input.kind)
                || !valid_finance_id(&input.idempotency_key)
                || input.reference.is_empty()
                || input.reference.len() > 256
                || input.amount == 0
            {
                return Err(anyhow!("invalid payment adjustment"));
            }
            let allowed = matches!(
                input.kind.as_str(),
                "refund" | "chargeback" | "dispute" | "manual_debit" | "manual_credit"
            );
            if !allowed || (input.amount > 0 && input.kind != "manual_credit") {
                return Err(anyhow!("unsupported payment adjustment kind"));
            }
            if serde_json::to_vec(&input.metadata)?.len() > 4096 {
                return Err(anyhow!("payment adjustment metadata is too large"));
            }
            let trx = state.trx();
            let request_hash = finance_hash(&serde_json::to_value(&input)?)?;
            let marker = format!("PaymentAdjustment::{}", input.idempotency_key);
            let previous = trx.get_link(&marker);
            if !previous.is_empty() {
                let Some((previous_hash, journal_id)) = previous.split_once('|') else {
                    return Err(anyhow!("invalid payment adjustment idempotency record"));
                };
                if previous_hash != request_hash {
                    return Err(anyhow!(
                        "idempotency key already used with different adjustment"
                    ));
                }
                return Ok(json!({
                    "applied": false, "alreadyApplied": true, "journalId": journal_id,
                    "account": financial_account_snapshot(&*trx, &input.user_id, 20)?,
                }));
            }
            let mut creature = Creature {
                id: input.user_id.clone(),
                ..Default::default()
            }
            .pull(&*trx);
            if creature.id.is_empty() {
                return Err(anyhow!("payment adjustment target not found"));
            }
            let old_debt = finance_debt_amount(&*trx, &input.user_id)?;
            let old_withdrawable = finance_withdrawable_amount(&*trx, &input.user_id)?;
            if old_withdrawable > creature.balance {
                return Err(anyhow!("withdrawable balance exceeds available balance"));
            }
            let (wallet_delta, debt_delta) = if input.amount < 0 {
                let reversal = input
                    .amount
                    .checked_abs()
                    .ok_or_else(|| anyhow!("payment adjustment overflow"))?;
                let available_debit = creature.balance.min(reversal);
                let debt_added = reversal
                    .checked_sub(available_debit)
                    .ok_or_else(|| anyhow!("payment adjustment underflow"))?;
                creature.balance = creature
                    .balance
                    .checked_sub(available_debit)
                    .ok_or_else(|| anyhow!("wallet adjustment underflow"))?;
                set_finance_withdrawable_amount(
                    &*trx,
                    &input.user_id,
                    old_withdrawable.min(creature.balance),
                )?;
                set_finance_debt_amount(
                    &*trx,
                    &input.user_id,
                    old_debt
                        .checked_add(debt_added)
                        .ok_or_else(|| anyhow!("wallet debt overflow"))?,
                )?;
                (-available_debit, debt_added)
            } else {
                let debt_repaid = old_debt.min(input.amount);
                let wallet_credit = input
                    .amount
                    .checked_sub(debt_repaid)
                    .ok_or_else(|| anyhow!("payment adjustment underflow"))?;
                creature.balance = creature
                    .balance
                    .checked_add(wallet_credit)
                    .ok_or_else(|| anyhow!("wallet balance overflow"))?;
                set_finance_debt_amount(&*trx, &input.user_id, old_debt - debt_repaid)?;
                (wallet_credit, -debt_repaid)
            };
            creature.push(&*trx);
            let participants = vec![input.user_id.clone(), state.info().user_id()];
            let journal_id = write_finance_journal(
                &*trx,
                &format!("payment.{}", input.kind),
                "",
                &input.user_id,
                json!({
                    "entries": [
                        {"account": format!("wallet:{}:available", input.user_id), "amount": wallet_delta},
                        {"account": format!("wallet:{}:debt", input.user_id), "amount": debt_delta},
                        {"account": "external:payments", "amount": -input.amount}
                    ],
                    "adjustmentAmount": input.amount, "kind": input.kind,
                    "reference": input.reference, "metadata": input.metadata,
                }),
                &participants,
                Utc::now().timestamp_millis(),
            )?;
            trx.put_link(&marker, &format!("{request_hash}|{journal_id}"));
            Ok(json!({
                "applied": true, "journalId": journal_id,
                "account": financial_account_snapshot(&*trx, &input.user_id, 20)?,
            }))
        },
    )
}

fn lock_token(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<LockTokenInput, _>(
        app,
        "/creatures/lockToken",
        user_guard(),
        move |state: Arc<dyn IState>, input: LockTokenInput| -> Result<Value> {
            let trx = state.trx();
            // Balance authority is the Creature record (same as transfer/mint).
            let mut user = Creature {
                id: state.info().user_id(),
                ..Default::default()
            }
            .pull(&*trx);

            let mut steps: Vec<Value> = Vec::with_capacity(input.steps.len().max(1));
            if !input.steps.is_empty() {
                for (i, step) in input.steps.iter().enumerate() {
                    if step.amount <= 0 {
                        return Err(anyhow!("step {} amount must be greater than zero", i));
                    }
                    if step.unlock_at <= 0 {
                        return Err(anyhow!(
                            "step {} unlockAt must be a unix timestamp in milliseconds",
                            i
                        ));
                    }
                    steps.push(json!({
                        "amount": step.amount,
                        "unlockAt": step.unlock_at,
                        "consumed": false,
                    }));
                }
            } else {
                if input.amount <= 0 {
                    return Err(anyhow!("amount must be greater than zero"));
                }
                if input.unlock_at <= 0 {
                    return Err(anyhow!("unlockAt must be a unix timestamp in milliseconds"));
                }
                steps.push(json!({
                    "amount": input.amount,
                    "unlockAt": input.unlock_at,
                    "consumed": false,
                }));
            }

            let total_amount = steps.iter().try_fold(0_i64, |total, step| {
                let amount = step.get("amount").and_then(|v| v.as_i64()).unwrap_or(0);
                total
                    .checked_add(amount)
                    .ok_or_else(|| anyhow!("lock amount overflow"))
            })?;
            if user.balance < total_amount {
                return Err(anyhow!("your balance is not enough"));
            }
            let lock_id = secure_unique_string();
            if input.typ == "pay" {
                if !trx.has_obj("Creature", &input.target) {
                    return Err(anyhow!("target user not acceptable"));
                }
                user.balance = user
                    .balance
                    .checked_sub(total_amount)
                    .ok_or_else(|| anyhow!("balance underflow"))?;
                user.push(&*trx);
                let payload = json!({
                    "type": "pay",
                    "amount": total_amount,
                    "remainingAmount": total_amount,
                    "userId": input.target,
                    "steps": steps,
                });
                trx.put_json(
                    &format!("Json::Creature::{}", state.info().user_id()),
                    &format!("lockedTokens.{}", lock_id),
                    &payload,
                    true,
                )?;
            } else {
                return Err(anyhow!("unknown lock type"));
            }
            Ok(json!({"tokenId": lock_id}))
        },
    )
}

fn consume_lock(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    let app_for_handler = app.clone();
    build_secure_action::<ConsumeLockInput, _>(
        app,
        "/creatures/consumeLock",
        user_guard(),
        move |state: Arc<dyn IState>, input: ConsumeLockInput| -> Result<Value> {
            let trx = state.trx();
            // Balance authority is the Creature record (same as transfer/mint).
            let mut receiver = Creature {
                id: state.info().user_id(),
                ..Default::default()
            }
            .pull(&*trx);
            if input.typ != "pay" {
                return Err(anyhow!("unknown lock type"));
            }
            if !trx.has_obj("Creature", &input.user_id) {
                return Err(anyhow!("payer user not found"));
            }
            let sender = Creature {
                id: input.user_id.clone(),
                ..Default::default()
            }
            .pull(&*trx);
            let payment_map = match trx.get_json(
                &format!("Json::Creature::{}", sender.id),
                &format!("lockedTokens.{}", input.lock_id),
            ) {
                Ok(m) => m,
                Err(_) => return Err(anyhow!("lock not found")),
            };
            let mut payment: Map<String, Value> = payment_map;
            let steps_raw = match payment.get("steps") {
                Some(Value::Array(arr)) if !arr.is_empty() => arr.clone(),
                _ => return Err(anyhow!("lock does not include steps")),
            };
            let mut step_index: i64 = input.step.unwrap_or(-1);
            let now = Utc::now().timestamp_millis();
            let mut parsed_steps: Vec<Map<String, Value>> = Vec::with_capacity(steps_raw.len());
            let mut parsed_amounts: Vec<i64> = Vec::with_capacity(steps_raw.len());
            let mut parsed_unlocks: Vec<i64> = Vec::with_capacity(steps_raw.len());
            for raw_step in steps_raw.iter() {
                let step_map = match raw_step {
                    Value::Object(o) => o.clone(),
                    _ => return Err(anyhow!("invalid lock step")),
                };
                let step_amount = step_map.get("amount").and_then(as_i64).unwrap_or(0);
                if step_amount <= 0 {
                    return Err(anyhow!("invalid lock step amount"));
                }
                let unlock_at = step_map.get("unlockAt").and_then(as_i64).unwrap_or(0);
                if unlock_at <= 0 {
                    return Err(anyhow!("invalid lock step unlockAt"));
                }
                let consumed = step_map
                    .get("consumed")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                parsed_steps.push(step_map);
                parsed_amounts.push(step_amount);
                parsed_unlocks.push(unlock_at);
                if step_index == -1 && !consumed && now >= unlock_at && step_amount == input.amount
                {
                    step_index = (parsed_steps.len() - 1) as i64;
                }
            }
            if step_index < 0 || (step_index as usize) >= parsed_steps.len() {
                return Err(anyhow!("lock step not found"));
            }
            let idx = step_index as usize;
            let selected_step = &mut parsed_steps[idx];
            let selected_amount = parsed_amounts[idx];
            let selected_unlock_at = parsed_unlocks[idx];
            if now < selected_unlock_at {
                return Err(anyhow!("lock step is not consumable yet"));
            }
            if selected_step
                .get("consumed")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                return Err(anyhow!("lock step already consumed"));
            }
            if input.amount != selected_amount {
                return Err(anyhow!("amount of payment not matched"));
            }
            let sign_payload = format!(
                "{}:{}:{}:{}:{}",
                input.lock_id, idx, selected_unlock_at, selected_amount, receiver.id
            );
            let (success, _, _) = app_for_handler.tools().security().auth_with_signature(
                &input.user_id,
                sign_payload.as_bytes(),
                &input.signature,
            );
            if !success {
                return Err(anyhow!("signature not verified"));
            }
            let typ = payment.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if typ != "pay" {
                return Err(anyhow!("type is not payment"));
            }
            let target = payment.get("userId").and_then(|v| v.as_str()).unwrap_or("");
            if target != receiver.id {
                return Err(anyhow!("you are not target"));
            }
            selected_step.insert("consumed".to_string(), Value::Bool(true));
            selected_step.insert("consumedAt".to_string(), json!(now));
            receiver.balance = receiver
                .balance
                .checked_add(input.amount)
                .ok_or_else(|| anyhow!("receiver balance overflow"))?;
            receiver.push(&*trx);
            let mut remaining_amount: i64 = 0;
            for (i, step_map) in parsed_steps.iter().enumerate() {
                let consumed = step_map
                    .get("consumed")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if !consumed {
                    remaining_amount = remaining_amount
                        .checked_add(parsed_amounts[i])
                        .ok_or_else(|| anyhow!("remaining lock amount overflow"))?;
                }
            }
            if remaining_amount == 0 {
                trx.del_json(
                    &format!("Json::Creature::{}", sender.id),
                    &format!("lockedTokens.{}", input.lock_id),
                );
            } else {
                let total_amount = payment.get("amount").and_then(as_i64).unwrap_or(0);
                if total_amount <= 0 {
                    return Err(anyhow!("invalid lock total amount"));
                }
                let steps_value: Value = Value::Array(
                    parsed_steps
                        .iter()
                        .map(|m| Value::Object(m.clone()))
                        .collect(),
                );
                payment.insert("steps".to_string(), steps_value);
                payment.insert("remainingAmount".to_string(), json!(remaining_amount));
                payment.insert(
                    "consumedAmount".to_string(),
                    json!(total_amount - remaining_amount),
                );
                trx.put_json(
                    &format!("Json::Creature::{}", sender.id),
                    &format!("lockedTokens.{}", input.lock_id),
                    &Value::Object(payment),
                    true,
                )?;
            }
            Ok(json!({
                "success": true,
                "step": idx,
                "remainingAmount": remaining_amount,
            }))
        },
    )
}

fn login(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    let app_for_handler = app.clone();
    build_secure_action::<LoginInput, _>(
        app,
        "/creatures/login",
        anon_guard(),
        move |state: Arc<dyn IState>, input: LoginInput| -> Result<Value> {
            // DEV-mode Firebase-Auth fallback. The Go module optionally
            // verified the supplied emailToken with the Firebase Admin SDK;
            // the Rust workspace doesn't carry a Firebase dependency so this
            // port short-circuits straight to the DEV path. Treat the token as
            // the raw email or fall back to a synthetic `username@dev.local`.
            let mut email = input.email_token.trim().to_string();
            if email.is_empty() || !email.contains('@') {
                email = format!("{}@dev.local", input.username);
            }
            log::info!(
                "[DEV] firebase disabled; accepting login for email: {}",
                email
            );

            let trx = state.trx();
            let user_id = trx.get_link(&format!("UserEmailToId::{}", email));
            if !user_id.is_empty() {
                let user = Creature {
                    id: user_id.clone(),
                    ..Default::default()
                }
                .pull(&*trx);
                if !user.id.is_empty() {
                    let session_id = trx.get_index("Session", "userId", "id", &user.id);
                    let session = Session {
                        id: session_id,
                        ..Default::default()
                    }
                    .pull(&*trx);
                    let private_key = trx.get_link(&format!("UserPrivateKey::{}", user.id));
                    return Ok(serde_json::to_value(LoginOutput {
                        user,
                        session,
                        private_key,
                    })?);
                }
                // Stale email link (creature was deleted but UserEmailToId was
                // not). Drop it so this login mints a new identity.
                trx.del_key(&format!("link::UserEmailToId::{}", email));
            }
            let expected_username = format!("{}@{}", input.username, app_for_handler.id());
            if trx.has_index("Creature", "username", "id", &expected_username) {
                return Err(anyhow!("username already exist"));
            }
            let (priv_raw, pub_raw) = secure_key_pairs("")?;
            let priv_key = String::from_utf8_lossy(&priv_raw).into_owned();
            let pub_key = String::from_utf8_lossy(&pub_raw).into_owned();
            let create_input = CreatureCreateInput {
                typ: "human".to_string(),
                username: input.username.clone(),
                public_key: pub_key,
                metadata: input.metadata.clone(),
                ..Default::default()
            };
            // Call /creatures/create's action directly on the current state.
            // Going through the secured chain re-submits the request and
            // deadlocks the chain processor (single-threaded), exactly like
            // the Go side.
            let create_action = app_for_handler
                .actor()
                .fetch_action("/creatures/create")
                .ok_or_else(|| anyhow!("/creatures/create not registered"))?;
            let typed_input: Arc<dyn IInput> = Arc::new(create_input);
            let (_code, res) = create_action.act(state.clone(), typed_input)?;
            let creature: Creature = res
                .get("creature")
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default();
            let session: Session = res
                .get("session")
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default();
            trx.put_link(&format!("UserPrivateKey::{}", creature.id), &priv_key);
            trx.put_link(&format!("UserEmailToId::{}", email), &creature.id);
            trx.put_link(&format!("UserIdToEmail::{}", creature.id), &email);
            Ok(serde_json::to_value(LoginOutput {
                user: creature,
                session,
                private_key: priv_key,
            })?)
        },
    )
}

fn delete(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<DeleteInput, _>(
        app,
        "/creatures/delete",
        user_guard(),
        move |state: Arc<dyn IState>, input: DeleteInput| -> Result<Value> {
            if input.user_id != state.info().user_id() && state.info().user_id() != "1@global" {
                return Err(anyhow!("access denied"));
            }
            let trx = state.trx();
            let user = Creature {
                id: input.user_id.clone(),
                ..Default::default()
            }
            .pull(&*trx);
            if user.id.is_empty() {
                return Err(anyhow!("user not found"));
            }
            user.delete(&*trx);
            trx.del_json(&format!("UserMeta::{}", input.user_id), "metadata");
            let store_list = Store::list(
                &*trx,
                &format!("hasaccess::{}::", input.user_id),
                false,
                &HashMap::new(),
                &HashMap::new(),
                -1,
                -1,
            )
            .unwrap_or_default();
            for store in &store_list {
                trx.del_key(&format!("link::onaccess::{}::{}", store.id, input.user_id));
                trx.del_key(&format!("link::hasaccess::{}::{}", input.user_id, store.id));
                trx.del_key(&format!("link::creatorof::{}::{}", input.user_id, store.id));
                let prefix = format!("onaccess::{}::", store.id);
                let remaining = trx.get_links_list(&prefix, -1, -1, &[]).unwrap_or_default();
                let others = remaining.iter().any(|k| {
                    let member = k.strip_prefix(&prefix).unwrap_or(k);
                    !member.is_empty() && member != input.user_id
                });
                if !others {
                    store.delete(&*trx);
                }
            }
            let email = trx.get_link(&format!("UserIdToEmail::{}", input.user_id));
            trx.del_key(&format!("link::UserIdToEmail::{}", input.user_id));
            if !email.is_empty() {
                trx.del_key(&format!("link::UserEmailToId::{}", email));
            }
            trx.del_key(&format!("link::UserPrivateKey::{}", input.user_id));
            Ok(json!({}))
        },
    )
}

fn update(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<UpdateInput, _>(
        app,
        "/creatures/update",
        user_guard(),
        move |state: Arc<dyn IState>, input: UpdateInput| -> Result<Value> {
            if input.user_id != state.info().user_id() && state.info().user_id() != "1@global" {
                return Err(anyhow!("access denied"));
            }
            let trx = state.trx();
            let mut creature = Creature {
                id: input.user_id.clone(),
                ..Default::default()
            }
            .pull(&*trx);
            if creature.id.is_empty() {
                return Err(anyhow!("user not found"));
            }
            if let Some(pk) = input.public_key.as_ref() {
                creature.public_key = pk.clone();
            }
            if let Some(t) = input.typ.as_ref() {
                creature.type_name = t.clone();
            }
            if let Some(name) = input.username.as_ref() {
                let base_username = creature
                    .username
                    .split('@')
                    .next()
                    .unwrap_or("")
                    .to_string();
                if *name != base_username {
                    let next_username = format!("{}@{}", name, state.source());
                    if trx.has_index("Creature", "username", "id", &next_username) {
                        return Err(anyhow!("username already exists"));
                    }
                    trx.del_index("Creature", "username", "id", &creature.username);
                    creature.username = next_username;
                }
            }
            creature.push(&*trx);
            Ok(json!({}))
        },
    )
}

fn meta(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<MetaInput, _>(
        app,
        "/creatures/meta",
        user_guard(),
        move |state: Arc<dyn IState>, input: MetaInput| -> Result<Value> {
            let trx = state.trx();
            let user = Creature {
                id: input.user_id.clone(),
                ..Default::default()
            }
            .pull(&*trx);
            if user.id.is_empty() {
                return Err(anyhow!("user not found"));
            }
            let m = trx
                .get_json(&format!("UserMeta::{}", input.user_id), "metadata")
                .unwrap_or_default();
            Ok(Value::Object(m))
        },
    )
}

fn apply_extender_fields(
    state: &Arc<dyn IState>,
    trx: &dyn crate::models::transaction::ITrx,
    user_id: &str,
    mut user_map: HashMap<String, Value>,
    extender: &HashMap<String, ExtendedField>,
) -> HashMap<String, Value> {
    // Mirrors the Go `for name, field := range ex { ... }` loop in
    // `creature/creature.go`:
    //   * if the field exposes a `GetValue` callback, invoke it with
    //     `(state, current_map)` and store the result;
    //   * otherwise pull the field from the user's metadata document at
    //     `field.path`, falling back to the declared default.
    // `trx.get_json` always returns the *object* at the given JSON path
    // (`serde_json::Map<String, Value>`), so we extract the specific key
    // from that object to obtain a single `Value`.
    for (key, field) in extender {
        if let Some(get_value) = &field.get_value {
            let snapshot: serde_json::Map<String, Value> = user_map.clone().into_iter().collect();
            if let Ok(v) = get_value(state.clone(), snapshot) {
                user_map.insert(key.clone(), v);
                continue;
            }
        }
        let value = trx
            .get_json(&format!("UserMeta::{}", user_id), &field.path)
            .ok()
            .and_then(|m| m.get(key).cloned())
            .unwrap_or_else(|| field.default.clone());
        user_map.insert(key.clone(), value);
    }
    user_map
}

fn get_by_username(
    app: Arc<dyn ICore>,
    user_extender: HashMap<String, ExtendedField>,
) -> Arc<dyn ISecureAction> {
    build_secure_action::<GetByUsernameInput, _>(
        app,
        "/creatures/getByUsername",
        user_guard(),
        move |state: Arc<dyn IState>, input: GetByUsernameInput| -> Result<Value> {
            let trx = state.trx();
            let user_id = trx.get_index("Creature", "username", "id", &input.username);
            if user_id.is_empty() {
                return Err(anyhow!("user not found"));
            }
            let result = Creature {
                id: user_id,
                ..Default::default()
            }
            .pull(&*trx);
            let m = object_to_map(&result).unwrap_or_default();
            let user_map: HashMap<String, Value> = m.into_iter().collect();
            let user_map =
                apply_extender_fields(&state, &*trx, &result.id, user_map, &user_extender);
            Ok(serde_json::to_value(GetOutput { user: user_map })?)
        },
    )
}

fn find(
    app: Arc<dyn ICore>,
    user_extender: HashMap<String, ExtendedField>,
) -> Arc<dyn ISecureAction> {
    build_secure_action::<FindInput, _>(
        app,
        "/creatures/find",
        user_guard(),
        move |state: Arc<dyn IState>, input: FindInput| -> Result<Value> {
            let trx = state.trx();
            let users = Creature::search(&*trx, 0, 1, "username", &input.username, &HashMap::new())
                .unwrap_or_default();
            if users.is_empty() {
                return Err(anyhow!("user not found"));
            }
            let result = users.into_iter().next().unwrap();
            let m = object_to_map(&result).unwrap_or_default();
            let user_map: HashMap<String, Value> = m.into_iter().collect();
            let user_map =
                apply_extender_fields(&state, &*trx, &result.id, user_map, &user_extender);
            Ok(serde_json::to_value(GetOutput { user: user_map })?)
        },
    )
}

/// List the registered creature types (their specs). Lets the host inspect the
/// extensible type registry.
fn types(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<ListInput, _>(
        app,
        "/creatures/types",
        user_guard(),
        move |state: Arc<dyn IState>, _input: ListInput| -> Result<Value> {
            let trx = state.trx();
            let prefix = "CreatureTypeExists::";
            let links = trx.get_links_list(prefix, -1, -1, &[]).unwrap_or_default();
            let mut out: Vec<Value> = Vec::new();
            for link in links {
                let name = link.strip_prefix(prefix).unwrap_or(&link).to_string();
                if let Some(mut spec) = get_creature_type(&*trx, &name) {
                    spec.insert("name".to_string(), json!(name));
                    out.push(Value::Object(spec));
                }
            }
            Ok(json!({ "types": out }))
        },
    )
}

/// Install every creature action onto the actor.
pub fn install(
    app: Arc<dyn ICore>,
    model_extender: HashMap<String, HashMap<String, ExtendedField>>,
) {
    let user_extender = model_extender.get("user").cloned().unwrap_or_default();
    // Bootstrap phase: register the built-in creature types (idempotent).
    install_creature_types(app.clone());
    let actor = app.actor();
    let handlers: Vec<Arc<dyn ISecureAction>> = vec![
        create(app.clone()),
        get(app.clone()),
        list(app.clone()),
        transfer(app.clone()),
        signal(app.clone()),
        authenticate(app.clone()),
        mint(app.clone()),
        check_sign(app.clone()),
        secret_put(app.clone()),
        secret_get(app.clone()),
        secret_grant(app.clone()),
        secret_revoke(app.clone()),
        start_hold(app.clone()),
        secret_list(app.clone()),
        secret_list_granted(app.clone()),
        storage_upload(app.clone()),
        publish_finance_catalog(app.clone()),
        register_finance_node(app.clone()),
        retire_finance_node(app.clone()),
        register_finance_resource(app.clone()),
        review_finance_resource(app.clone()),
        retire_finance_resource(app.clone()),
        publish_finance_quote(app.clone()),
        create_hold(app.clone()),
        settle_hold(app.clone()),
        release_hold(app.clone()),
        get_hold(app.clone()),
        get_financial_account(app.clone()),
        request_payout(app.clone()),
        resolve_payout(app.clone()),
        list_payouts(app.clone()),
        reconcile_financial_system(app.clone()),
        payment_adjustment(app.clone()),
        lock_token(app.clone()),
        consume_lock(app.clone()),
        login(app.clone()),
        delete(app.clone()),
        update(app.clone()),
        meta(app.clone()),
        get_by_username(app.clone(), user_extender.clone()),
        find(app.clone(), user_extender.clone()),
        types(app.clone()),
    ];
    for h in handlers {
        actor.inject_secure_action(h);
    }
}
