//! Translation of `abstract/models/action/actor.go`.

use std::sync::Arc;

use crate::abstractions::models::action::{IAction, ISecureAction};
use crate::util::AnyVal;

/// Registry of actions and services.
///
/// Rust can't downcast `Arc<dyn IAction>` to `Arc<dyn ISecureAction>` the way
/// Go's type assertion does, so secured actions are registered via the
/// dedicated `inject_secure_action` channel and looked up via
/// `fetch_secure_action`. The Go shell unconditionally type-asserted every
/// action to `ISecureAction`; the Rust shell calls `inject_secure_action`
/// when registering a `SecureAction` so the same lookups work.
pub trait IActor: Send + Sync {
    fn inject_action(&self, action: Arc<dyn IAction>);
    fn inject_service(&self, service: AnyVal);
    fn fetch_action(&self, key: &str) -> Option<Arc<dyn IAction>>;
    fn inject_secure_action(&self, action: Arc<dyn ISecureAction>);
    fn fetch_secure_action(&self, key: &str) -> Option<Arc<dyn ISecureAction>>;
}
