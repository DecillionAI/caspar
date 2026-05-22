//! Translation of `shell/api/inputs/users` — request payload structs the
//! user-management actions consume. Only the inputs other drivers already
//! depend on are translated here; the rest follow when the action handlers
//! land.

use serde::{Deserialize, Serialize};

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
}
