//! Translation of `abstract/models/packet/consume.go`.

use serde::{Deserialize, Serialize};

/// Input for consuming an amount of a token. In Go the fields carry
/// `validate:"required"` tags; validation is applied by the shell API layer.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConsumeTokenInput {
    #[serde(rename = "orig")]
    pub orig: String,
    #[serde(rename = "tokenOwnerId")]
    pub token_owner_id: String,
    #[serde(rename = "tokenId")]
    pub token_id: String,
    #[serde(rename = "amount")]
    pub amount: i64,
}
