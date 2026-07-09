//! Runtime models for the wasm VM plugin: the host-backed write-ahead
//! transaction used by the raw `dbOp` host calls.

use std::collections::BTreeMap;

use caspar_vm_sdk::host::{host, log, KvOp};

#[derive(Clone)]
pub struct WasmDbOp {
    pub type_: String,
    pub key: String,
    pub val: String,
}

/// Per-execution write-ahead buffer for raw link-level db ops.
///
/// Reads are served from the local overlay first (read-your-own-writes) and
/// fall through to the node state via the SDK host interface; writes
/// accumulate locally and are committed atomically through
/// `VmHost::state_apply_ops`.
pub struct Trx {
    pub store: BTreeMap<String, String>,
    pub newly_created: BTreeMap<String, bool>,
    pub newly_deleted: BTreeMap<String, bool>,
    pub ops: Vec<WasmDbOp>,
}

impl Trx {
    pub fn new() -> Self {
        Trx {
            store: BTreeMap::new(),
            newly_created: BTreeMap::new(),
            newly_deleted: BTreeMap::new(),
            ops: Vec::new(),
        }
    }

    pub fn put(&mut self, key: String, val: String) {
        self.ops.push(WasmDbOp {
            type_: "put".to_string(),
            key: key.clone(),
            val: val.clone(),
        });
        self.store.insert(key.clone(), val);
        self.newly_created.insert(key.clone(), true);
        self.newly_deleted.remove(&key);
    }

    pub fn get_by_prefix(&mut self, prefix: String) -> Vec<String> {
        // First collect any locally-buffered values matching the prefix.
        let mut result: Vec<String> = self
            .store
            .iter()
            .filter(|(k, _)| k.starts_with(&prefix))
            .map(|(_, v)| v.clone())
            .collect();

        let fetched = match host() {
            Some(h) => h.state_get_by_prefix(&prefix),
            None => Vec::new(),
        };
        for v in fetched {
            if !result.contains(&v) {
                result.push(v);
            }
        }
        result
    }

    pub fn get(&mut self, key: String) -> String {
        if let Some(value) = self.store.get(&key) {
            return value.clone();
        }
        if self.newly_deleted.contains_key(&key) {
            return "".to_string();
        }
        let value = match host() {
            Some(h) => h.state_get(&key),
            None => String::new(),
        };
        self.store.insert(key, value.clone());
        value
    }

    pub fn del(&mut self, key: String) {
        let k = key;
        self.ops.push(WasmDbOp {
            type_: "del".to_string(),
            key: k.clone(),
            val: String::new(),
        });
        self.store.remove(&k);
        self.newly_created.remove(&k);
        self.newly_deleted.insert(k, true);
    }

    /// Commit all buffered ops atomically through the node's consensus-aware
    /// state layer.
    pub fn commit_as_offchain(&mut self) {
        if self.ops.is_empty() {
            return;
        }
        let ops: Vec<KvOp> = self
            .ops
            .iter()
            .map(|o| KvOp {
                op: o.type_.clone(),
                key: o.key.clone(),
                val: o.val.clone(),
            })
            .collect();
        if let Some(h) = host() {
            if let Err(e) = h.state_apply_ops(&ops) {
                log(format!("wasm trx commit failed: {}", e));
                return;
            }
            self.ops.clear();
            log("committed transaction successfully.".to_string());
        }
    }
}
