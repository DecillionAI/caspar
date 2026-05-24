//! Request and response payloads for the `users` action namespace.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::abstractions::models::input::IInput;
use crate::shell::api::model::{Session, User};

macro_rules! input_impls {
    ($($t:ty => $origin:expr),* $(,)?) => {
        $(
            impl IInput for $t {
                fn get_store_id(&self) -> String { String::new() }
                fn origin(&self) -> String { $origin.to_string() }
                fn as_any(&self) -> &dyn std::any::Any { self }
            }
        )*
    };
}

fn i64_is_zero(n: &i64) -> bool {
    *n == 0
}

// ---- Inputs ----------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConsumeLockInput {
    #[serde(rename = "type", default)]
    pub typ: String,
    #[serde(rename = "userId", default)]
    pub user_id: String,
    #[serde(rename = "lockId", default)]
    pub lock_id: String,
    #[serde(default)]
    pub signature: String,
    #[serde(default)]
    pub amount: i64,
    /// Optional step index inside a multi-step lock. Matches Go's `*int`
    /// (`nil` = auto-pick the next consumable step).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GetInput {
    #[serde(rename = "userId", default)]
    pub user_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FindInput {
    #[serde(default)]
    pub username: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListInput {
    #[serde(default)]
    pub offset: i64,
    #[serde(default)]
    pub count: i64,
    #[serde(default)]
    pub param: String,
    #[serde(default)]
    pub query: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MetaInput {
    #[serde(rename = "userId", default)]
    pub user_id: String,
    #[serde(default)]
    pub path: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeleteInput {
    #[serde(rename = "userId", default)]
    pub user_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CheckSignInput {
    #[serde(rename = "userId", default)]
    pub user_id: String,
    #[serde(default)]
    pub payload: String,
    #[serde(default)]
    pub signature: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthenticateInput {}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GetByUsernameInput {
    #[serde(default)]
    pub username: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreateInput {
    #[serde(default)]
    pub username: String,
    #[serde(rename = "publicKey", default)]
    pub public_key: String,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MintInput {
    #[serde(rename = "toUserEmail", default)]
    pub to_user_email: String,
    #[serde(default)]
    pub amount: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LockTokenInput {
    #[serde(default, skip_serializing_if = "i64_is_zero")]
    pub amount: i64,
    #[serde(rename = "type", default)]
    pub typ: String,
    #[serde(default)]
    pub target: String,
    #[serde(rename = "unlockAt", default, skip_serializing_if = "i64_is_zero")]
    pub unlock_at: i64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<LockTokenStepInput>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LockTokenStepInput {
    #[serde(default)]
    pub amount: i64,
    #[serde(rename = "unlockAt", default)]
    pub unlock_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TransferInput {
    #[serde(rename = "toUsername", default)]
    pub to_username: String,
    #[serde(default)]
    pub amount: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateInput {
    #[serde(rename = "userId", default)]
    pub user_id: String,
    #[serde(default)]
    pub metadata: Value,
    #[serde(rename = "publicKey", default, skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub typ: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LoginInput {
    #[serde(default)]
    pub username: String,
    #[serde(rename = "emailToken", default)]
    pub email_token: String,
    #[serde(default)]
    pub metadata: Value,
}

input_impls! {
    ConsumeLockInput => "global",
    GetInput         => "",
    FindInput        => "",
    ListInput        => "",
    MetaInput        => "global",
    DeleteInput      => "global",
    CheckSignInput   => "",
    AuthenticateInput => "",
    GetByUsernameInput => "",
    CreateInput      => "global",
    MintInput        => "global",
    LockTokenInput   => "global",
    TransferInput    => "global",
    UpdateInput      => "global",
    LoginInput       => "",
}

// ---- Outputs ---------------------------------------------------------------

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
