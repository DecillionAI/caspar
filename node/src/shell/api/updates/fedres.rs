//! Translation of `shell/api/updates/fedres.go`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FedRes {
    #[serde(rename = "requestId", default)]
    pub request_id: String,
    #[serde(default)]
    pub data: String,
}
