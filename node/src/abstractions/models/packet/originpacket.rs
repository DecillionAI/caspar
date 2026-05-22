//! Translation of `abstract/models/packet/originpacket.go`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct OriginPacket {
    #[serde(rename = "Type")]
    pub typ: String,
    pub key: String,
    pub user_id: String,
    pub store_id: String,
    pub request_id: String,
    pub res_code: i64,
    #[serde(with = "crate::util::bytes_base64", default)]
    pub binary: Vec<u8>,
    pub signature: String,
    #[serde(default)]
    pub exceptions: Vec<String>,
}
