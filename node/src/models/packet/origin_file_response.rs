use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct OriginFileRes {
    pub user_id: String,
    pub store_id: String,
    pub request_id: String,
    pub file_id: String,
}
