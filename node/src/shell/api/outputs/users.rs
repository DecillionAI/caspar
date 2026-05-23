//! Translation of `shell/api/outputs/users`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::shell::api::model::{Session, User};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthenticateOutput {
    #[serde(default)]
    pub authenticated: bool,
    #[serde(default)]
    pub user: HashMap<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreateOutput {
    #[serde(default)]
    pub user: User,
    #[serde(default)]
    pub session: Session,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GetOutput {
    #[serde(default)]
    pub user: HashMap<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LoginOutput {
    #[serde(default)]
    pub user: User,
    #[serde(default)]
    pub session: Session,
    #[serde(rename = "privateKey", default)]
    pub private_key: String,
}
