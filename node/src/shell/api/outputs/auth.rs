//! Translation of `shell/api/outputs/auth`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GetServerKeyOutput {
    #[serde(rename = "publicKey", default)]
    pub public_key: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GetServersMapOutput {
    #[serde(default)]
    pub servers: Vec<String>,
}
