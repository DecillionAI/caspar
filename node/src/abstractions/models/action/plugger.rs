//! Translation of `abstract/models/action/plugger.go`.

use std::sync::Arc;

use crate::abstractions::models::action::action::IActions;

/// A plugger exposes a set of actions to be installed onto the node.
pub trait IPlugger: Send + Sync {
    fn actions(&self) -> Arc<dyn IActions>;
}
