//! Translation of `shell/api/outputs/plugin`.

use serde::{Deserialize, Serialize};

use crate::shell::api::model::User;

/// `assign.go::AssignOutput` — empty body.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AssignOutput {}

/// `create.go::CreateOutput`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreateOutput {
    #[serde(default)]
    pub user: User,
}

/// `plug.go::PlugInput` — empty body (the Go file declared an "input" type
/// inside an outputs package; the name is preserved for parity).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlugInput {}
