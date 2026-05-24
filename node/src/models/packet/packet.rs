use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Packet {
    #[serde(rename = "origin")]
    pub origin: String,
    #[serde(rename = "data")]
    pub data: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LogPacket {
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "storeId")]
    pub store_id: String,
    #[serde(rename = "userId")]
    pub user_id: String,
    #[serde(rename = "data")]
    pub data: String,
    #[serde(rename = "time")]
    pub time: i64,
    #[serde(rename = "edited")]
    pub edited: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BuildPacket {
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "buildId")]
    pub build_id: String,
    #[serde(rename = "creatureId")]
    pub creature_id: String,
    #[serde(rename = "vmId", skip_serializing_if = "String::is_empty", default)]
    pub vm_id: String,
    #[serde(rename = "logType", skip_serializing_if = "String::is_empty", default)]
    pub log_type: String,
    #[serde(rename = "time")]
    pub time: i64,
    #[serde(rename = "data")]
    pub data: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::packet::command::Command;
    use crate::models::packet::error::build_error_json;

    #[test]
    fn test_build_error_json() {
        let err_obj = build_error_json("boom");
        assert_eq!(err_obj.message, "boom", "message mismatch");
    }

    #[test]
    fn test_packet_json_shapes() {
        let p = Packet { origin: "fed-a".to_string(), data: "payload".to_string() };
        let raw = serde_json::to_string(&p).expect("marshal packet");
        assert_eq!(raw, r#"{"origin":"fed-a","data":"payload"}"#, "unexpected packet json");

        let cmd = Command { value: "ping".to_string(), data: "x".to_string() };
        assert!(cmd.value == "ping" && cmd.data == "x", "command fields mismatch");
    }
}
