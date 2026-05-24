use serde::{Deserialize, Serialize};

/// Input for consuming an amount of a token. Required-field validation is
/// applied by the shell API layer.
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
