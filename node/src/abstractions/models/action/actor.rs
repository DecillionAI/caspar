//! Translation of `abstract/models/action/actor.go`.

use std::sync::Arc;

use crate::abstractions::models::action::action::IAction;
use crate::util::AnyVal;

/// Registry of actions and services.
pub trait IActor: Send + Sync {
    fn inject_action(&self, action: Arc<dyn IAction>);
    fn inject_service(&self, service: AnyVal);
    fn fetch_action(&self, key: &str) -> Option<Arc<dyn IAction>>;
}
