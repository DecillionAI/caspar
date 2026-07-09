//! Shared helpers used by the VMM and by every VM plugin.

use base64::Engine;
use serde_json::{json, Value};

/// Canonical runtime-name normalisation (`strings.ToLower(TrimSpace(.))`).
pub fn normalize_runtime(runtime: &str) -> String {
    runtime.trim().to_lowercase()
}

/// Resource limits attached to a VM execution request.
#[derive(Clone, Debug)]
pub struct VmResourceLimits {
    pub max_exec_time_secs: u64,
    pub ram_mb: u64,
    pub disk_gb: u64,
    pub cpu_cores: u64,
}

impl VmResourceLimits {
    pub fn with_defaults() -> Self {
        VmResourceLimits {
            max_exec_time_secs: 60,
            ram_mb: 64,
            disk_gb: 1,
            cpu_cores: 1,
        }
    }
}

/// Parse the `resources` object of a VM packet, falling back to defaults.
pub fn parse_vm_resource_limits(packet: &Value) -> VmResourceLimits {
    let mut limits = VmResourceLimits::with_defaults();
    let resources = &packet["resources"];
    if resources.is_object() {
        limits.max_exec_time_secs = resources["maxExecTimeSeconds"].as_u64().unwrap_or(60).max(1);
        limits.ram_mb = resources["ramMb"].as_u64().unwrap_or(64).max(1);
        limits.disk_gb = resources["diskGb"].as_u64().unwrap_or(1).max(1);
        limits.cpu_cores = resources["cpuCores"].as_u64().unwrap_or(1).max(1);
    }
    limits
}

/// Parse a JSON array field of u64s.
pub fn parse_u64_array_field(packet: &Value, field_name: &str) -> Vec<u64> {
    packet[field_name]
        .as_array()
        .map(|arr| arr.iter().filter_map(|v| v.as_u64()).collect())
        .unwrap_or_default()
}

/// Parse a byte-blob field: either a base64 string (compact — preferred for
/// large blobs like STARK proofs) or a raw JSON number array.
pub fn parse_u8_array_field(packet: &Value, field_name: &str) -> Vec<u8> {
    match &packet[field_name] {
        Value::String(s) => base64::engine::general_purpose::STANDARD
            .decode(s.as_bytes())
            .unwrap_or_default(),
        Value::Array(arr) => arr
            .iter()
            .filter_map(|v| v.as_u64().and_then(|n| u8::try_from(n).ok()))
            .collect(),
        _ => Vec::new(),
    }
}

/// Extract a human-readable message from a `catch_unwind` payload.
pub fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        return (*s).to_string();
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    "<non-string panic payload>".to_string()
}

/// Publish a uniform error packet for any VM runtime failure so the host
/// observer sees the failure instead of a silent drop.
pub fn emit_vm_error(machine_id: &str, vm_id: &str, runtime: &str, err: &str) {
    if let Some(h) = crate::host::host() {
        let _ = h.dispatch(&json!({
            "key": "vmOutput",
            "input": {
                "text": err,
                "data": err,
                "vmId": vm_id,
                "machineId": machine_id,
                "logType": "error",
                "runtime": runtime,
            }
        }));
    }
}
