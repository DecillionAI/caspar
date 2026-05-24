//! Request and response payloads for the `plugin` action namespace.

use serde::{Deserialize, Serialize};

use crate::shell::api::model::User;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AssignOutput {}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreateOutput {
    #[serde(default)]
    pub user: User,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlugInput {}
