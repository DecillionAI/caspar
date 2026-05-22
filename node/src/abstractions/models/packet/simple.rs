//! Translation of `abstract/models/packet/simple.go`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResponseSimpleMessage {
    #[serde(rename = "message")]
    pub message: String,
}
