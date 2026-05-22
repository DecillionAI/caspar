//! Translation of `core/module/actor/actor.go`.

use std::sync::{Arc, Mutex};

use dashmap::DashMap;

use crate::abstractions::models::action::action::IAction;
use crate::abstractions::models::action::actor::IActor;
use crate::util::AnyVal;

/// Concrete [`IActor`] registry — stores actions by `key()` and lets callers
/// look them up at runtime. `inject_service` is a no-op (matches Go), kept on
/// the trait for compatibility.
pub struct Actor {
    actions: DashMap<String, Arc<dyn IAction>>,
    services: Mutex<Vec<AnyVal>>,
}

impl Default for Actor {
    fn default() -> Self {
        Actor::new()
    }
}

impl Actor {
    /// Instantiate an empty `Actor`.
    pub fn new() -> Actor {
        Actor {
            actions: DashMap::new(),
            services: Mutex::new(Vec::new()),
        }
    }
}

impl IActor for Actor {
    fn inject_action(&self, action: Arc<dyn IAction>) {
        self.actions.insert(action.key(), action);
    }

    fn inject_service(&self, service: AnyVal) {
        // Go's implementation simply discarded the service; keep a copy so
        // tests can observe injection without changing externally-visible
        // semantics.
        self.services.lock().unwrap().push(service);
    }

    fn fetch_action(&self, key: &str) -> Option<Arc<dyn IAction>> {
        self.actions.get(key).map(|a| a.clone())
    }
}
