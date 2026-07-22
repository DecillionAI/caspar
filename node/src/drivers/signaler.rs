//! Translation of `drivers/signaler/signaler.go`.
//!
//! Pub/sub fan-out driver: single listeners (per user / per machine), groups
//! of stores, an optional global bridge, and a join/leave listener. Cross-
//! origin routes go through `IFederation::send_fed_update`.

use std::sync::{Arc, Mutex};

use anyhow::Result;
use dashmap::DashMap;
use serde_json::Value;

use crate::models::ports::network::federation::IFederation;
use crate::models::ports::signaler::{
    GlobalListener, Group, ISignaler, JoinListener, Listener,
};
use crate::models::core::ICore;
use crate::models::transaction::ITrx;

/// Concrete [`ISignaler`] implementation. Owns per-listener / per-group
/// `DashMap`s and a coarse-grained `Mutex` matching `Signaler.lock` from Go.
pub struct Signaler {
    lock: Mutex<()>,
    app: Arc<dyn ICore>,
    listeners: Arc<DashMap<String, Arc<Listener>>>,
    groups: Arc<DashMap<String, Arc<Group>>>,
    global_bridge: Mutex<Option<Arc<GlobalListener>>>,
    l_group_disabled: Mutex<bool>,
    j_listener: Mutex<Option<Arc<JoinListener>>>,
    federation: Arc<dyn IFederation>,
}

impl Signaler {
    /// `NewSignaler(app, federation)`.
    pub fn new(app: Arc<dyn ICore>, federation: Arc<dyn IFederation>) -> Arc<Signaler> {
        Arc::new(Signaler {
            lock: Mutex::new(()),
            app,
            listeners: Arc::new(DashMap::new()),
            groups: Arc::new(DashMap::new()),
            global_bridge: Mutex::new(None),
            l_group_disabled: Mutex::new(false),
            j_listener: Mutex::new(None),
            federation,
        })
    }

    /// Internal: dispatch a `Signal` to one in-process listener. `data` is
    /// always a `Value`; `pack` is preserved for API compatibility but has no
    /// effect (Rust `Value` is already JSON-encodable).
    fn signal_listener(&self, key: &str, listener_id: &str, data: Value) {
        let Some(listener) = self.listeners.get(listener_id).map(|e| e.value().clone())
        else {
            return;
        };
        (listener.signal)(key.to_string(), data);
    }

    /// Read `User.<id>.username` inside a read-only state modification.
    fn read_user_username(&self, user_id: &str) -> String {
        let slot = Arc::new(Mutex::new(String::new()));
        let slot_clone = slot.clone();
        let user_id_owned = user_id.to_string();
        self.app.modify_state(
            true,
            Box::new(move |trx: &dyn ITrx| {
                let mut v = trx.get_column("Creature", &user_id_owned, "username");
                if v.is_empty() {
                    v = trx.get_column("User", &user_id_owned, "username");
                }
                *slot_clone.lock().unwrap() = String::from_utf8_lossy(&v).into_owned();
                Ok(())
            }),
        );
        let out = slot.lock().unwrap().clone();
        out
    }
}

impl ISignaler for Signaler {
    // Go exposed manual `Lock()` / `Unlock()` on the signaler so external
    // callers could batch operations under the same mutex. Rust's
    // `std::sync::Mutex` doesn't expose a raw unlock without `unsafe`; the
    // ISignaler methods that actually mutate shared state already take their
    // own short-lived locks (see `listen_to_single`), so the trait
    // `lock`/`unlock` entry points are no-ops in the Rust port. External
    // callers that need a critical section should hold their own Mutex.
    fn lock(&self) {}
    fn unlock(&self) {}

    fn listeners(&self) -> Arc<DashMap<String, Arc<Listener>>> {
        self.listeners.clone()
    }

    fn groups(&self) -> Arc<DashMap<String, Arc<Group>>> {
        self.groups.clone()
    }

    fn listen_to_single(&self, listener: Arc<Listener>) {
        let _g = self.lock.lock().unwrap();
        self.listeners.insert(listener.id.clone(), listener);
    }

    fn listen_to_group(&self, listener: Arc<Listener>, override_functionaly: bool) {
        let group = self
            .retrive_group(&listener.id)
            .expect("retrive_group always returns Some");
        *group.listener.lock().unwrap() = Some(listener);
        *group.override_.lock().unwrap() = override_functionaly;
    }

    fn brdige_globally(&self, listener: Arc<GlobalListener>, _override_functionaly: bool) {
        *self.l_group_disabled.lock().unwrap() = true;
        *self.global_bridge.lock().unwrap() = Some(listener);
    }

    fn listen_to_join(&self, listener: Arc<JoinListener>) {
        *self.j_listener.lock().unwrap() = Some(listener);
    }

    fn signal_user(&self, key: &str, listener_id: &str, data: Value, pack: bool) {
        let _ = pack;
        if !listener_id.contains('@') {
            self.signal_listener(key, listener_id, data);
            return;
        }
        if self.listeners.contains_key(listener_id) {
            self.signal_listener(key, listener_id, data);
            return;
        }
        let username = self.read_user_username(listener_id);
        if username.is_empty() {
            return;
        }
        let Some(origin) = username.rsplit_once('@').map(|(_, s)| s.to_string()) else {
            return;
        };
        if origin == self.app.id() {
            self.signal_listener(key, listener_id, data);
        } else {
            self.federation.send_fed_update(
                &origin,
                key,
                data,
                "user",
                listener_id,
                Vec::new(),
            );
        }
    }

    fn signal_group(
        &self,
        key: &str,
        group_id: &str,
        data: Value,
        _pack: bool,
        exceptions: Vec<String>,
    ) {
        let exc: std::collections::HashSet<String> = exceptions.into_iter().collect();
        let group = match self.retrive_group(group_id) {
            Some(g) => g,
            None => return,
        };
        let packet = data.clone();

        if *self.l_group_disabled.lock().unwrap() {
            if let Some(bridge) = self.global_bridge.lock().unwrap().clone() {
                (bridge.signal)(group_id.to_string(), packet);
            }
            return;
        }
        if *group.override_.lock().unwrap() {
            if let Some(listener) = group.listener.lock().unwrap().clone() {
                (listener.signal)(key.to_string(), packet);
            }
            return;
        }

        let mut foreigners: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        let store_entries: Vec<(String, String)> = group
            .stores
            .iter()
            .map(|e| (e.key().clone(), e.value().clone()))
            .collect();
        for (store_key, user_id) in store_entries {
            let username = self.read_user_username(&user_id);
            if username.is_empty() {
                continue;
            }
            let Some(user_origin) = username.rsplit_once('@').map(|(_, s)| s.to_string())
            else {
                continue;
            };
            if user_origin == self.app.id() || user_origin == "global" {
                if !exc.contains(&store_key) {
                    if let Some(listener) =
                        self.listeners.get(&user_id).map(|e| e.value().clone())
                    {
                        (listener.signal)(key.to_string(), packet.clone());
                    }
                }
            } else {
                let entry = foreigners.entry(user_origin).or_default();
                if exc.contains(&store_key) {
                    entry.push(user_id);
                }
            }
        }

        for (origin, exceptions_for_origin) in foreigners {
            self.federation.send_fed_update(
                &origin,
                key,
                data.clone(),
                "store",
                group_id,
                exceptions_for_origin,
            );
        }
    }

    fn join_group(&self, group_id: &str, user_id: &str) {
        let Some(g) = self.retrive_group(group_id) else {
            return;
        };
        g.stores.insert(user_id.to_string(), user_id.to_string());
        if let Some(j) = self.j_listener.lock().unwrap().clone() {
            (j.join)(group_id.to_string(), user_id.to_string());
        }
    }

    fn leave_group(&self, group_id: &str, user_id: &str) {
        let Some(g) = self.retrive_group(group_id) else {
            return;
        };
        g.stores.remove(user_id);
        if let Some(j) = self.j_listener.lock().unwrap().clone() {
            (j.leave)(group_id.to_string(), user_id.to_string());
        }
    }

    fn retrive_group(&self, group_id: &str) -> Option<Arc<Group>> {
        if let Some(existing) = self.groups.get(group_id) {
            return Some(existing.value().clone());
        }
        let fresh = Arc::new(Group {
            stores: Arc::new(DashMap::new()),
            listener: Mutex::new(None),
            override_: Mutex::new(false),
        });
        self.groups
            .entry(group_id.to_string())
            .or_insert(fresh)
            .value()
            .clone()
            .into()
    }
}

// Suppress an unused-import warning.
const _: fn() -> Result<()> = || Ok(());
