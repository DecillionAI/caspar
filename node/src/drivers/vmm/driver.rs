//! Translation of `drivers/vmm/vmm.go`.
//!
//! Top-level `Vmm` struct, ZMQ REQ/REP loop, and the high-level public API
//! (`assign`, `run_vm`, `run_vm_entity`, `terminate_vm`, `build_vm_image`,
//! `close_kvdb`). Per-runtime hostcall handlers live in
//! [`hostcall_entities`](super::hostcall_entities) /
//! [`hostcall_logs`](super::hostcall_logs); the dispatcher lives in
//! [`hostcall_global`](super::hostcall_global).

use crate::drivers::vmm::dispatch_packet;
use std::fs;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde_json::{json, Value};

use crate::models::ports::file::IFile;
use crate::models::ports::signaler::Listener;
use crate::models::ports::storage::IStorage;
use crate::models::ports::vmm::IVmm;
use crate::models::core::ICore;
use crate::models::transaction::ITrx;
use crate::models::worker::Trx as WorkerTrx;
use crate::shell::api::model::{Program, Store, User};
use crate::shell::api::packets::stores;

/// Default appengine REP socket exposed *by* the node (the engine connects
/// to this with a REQ socket).

/// The virtual-machine driver.
pub struct Vmm {
    pub(super) app: Arc<dyn ICore>,
    pub(super) storage_root: String,
    pub(super) storage: Arc<dyn IStorage>,
    pub(super) file: Arc<dyn IFile>,
}

impl Vmm {
    /// `NewVmm(core, storageRoot, storage, kvDbPath, file)`.
    pub fn new(
        app: Arc<dyn ICore>,
        storage_root: &str,
        storage: Arc<dyn IStorage>,
        kv_db_path: &str,
        file: Arc<dyn IFile>,
    ) -> Arc<Vmm> {
        let _ = fs::create_dir_all(kv_db_path);
        let vmm = Arc::new(Vmm {
            app,
            storage_root: storage_root.to_string(),
            storage,
            file,
        });
        vmm
    }

    pub(super) fn send_to_engine(&self, value: Value) {
        let _ = dispatch_packet(&value);
    }

    /// `RunVm(machineId, storeId, data)`.
    pub fn run_vm_inner(self: &Arc<Self>, machine_id: &str, store_id: &str, data: &str) {
        self.run_vm_entity_inner(machine_id, store_id, data, "");
    }

    /// `RunVmEntity(machineId, storeId, data, entityId)`.
    pub fn run_vm_entity_inner(
        self: &Arc<Self>,
        machine_id: &str,
        store_id: &str,
        data: &str,
        entity_id: &str,
    ) {
        let store_id_owned = store_id.to_string();
        let machine_id_owned = machine_id.to_string();
        let store_slot = Arc::new(Mutex::new(Store::default()));
        let member_slot = Arc::new(Mutex::new(false));
        let store_clone = store_slot.clone();
        let member_clone = member_slot.clone();
        let store_id_clone = store_id_owned.clone();
        let machine_id_clone = machine_id_owned.clone();
        self.app.modify_state(
            true,
            Box::new(move |trx: &dyn ITrx| {
                let s = Store {
                    id: store_id_clone.clone(),
                    ..Default::default()
                }
                .pull(trx);
                *store_clone.lock().unwrap() = s;
                *member_clone.lock().unwrap() = trx.get_link(&format!(
                    "hasaccess::{}::{}",
                    machine_id_clone, store_id_clone
                )) == "true";
                Ok(())
            }),
        );
        if !*member_slot.lock().unwrap() {
            return;
        }
        let store = store_slot.lock().unwrap().clone();
        let (ast_path, vm_type) = self.resolve_vm_execution_target(machine_id, entity_id);
        let send_payload = stores::Send {
            user: User::default(),
            store,
            action: "single".to_string(),
            data: data.to_string(),
            ..Default::default()
        };
        let input = serde_json::to_string(&send_payload).unwrap_or_default();
        self.send_to_engine(json!({
            "type": "runVm",
            "machineId": machine_id,
            "input": input,
            "astPath": ast_path,
            "vmType": vm_type,
        }));
    }

    pub(super) fn resolve_vm_execution_target(
        &self,
        machine_id: &str,
        entity_id: &str,
    ) -> (String, String) {
        let default_path = format!(
            "{}/machines/{}/module",
            self.storage.storage_root(),
            machine_id
        );
        let path_slot = Arc::new(Mutex::new(default_path));
        let type_slot = Arc::new(Mutex::new("wasm".to_string()));
        let path_clone = path_slot.clone();
        let type_clone = type_slot.clone();
        let machine_id_owned = machine_id.to_string();
        let entity_id_owned = entity_id.to_string();
        self.app.modify_state(
            true,
            Box::new(move |trx: &dyn ITrx| {
                let vm = Program {
                    machine_id: machine_id_owned.clone(),
                    ..Default::default()
                }
                .pull(trx);
                if !vm.path.is_empty() {
                    *path_clone.lock().unwrap() = vm.path.clone();
                }
                if !vm.runtime.is_empty() {
                    *type_clone.lock().unwrap() = vm.runtime.trim().to_lowercase();
                }
                if !entity_id_owned.is_empty() {
                    let runtime_link = trx.get_link(&format!(
                        "vmEntityType::{}::{}",
                        machine_id_owned, entity_id_owned
                    ));
                    if !runtime_link.is_empty() {
                        *type_clone.lock().unwrap() = runtime_link.trim().to_lowercase();
                    }
                    let path_link = trx.get_link(&format!(
                        "vmEntityPath::{}::{}",
                        machine_id_owned, entity_id_owned
                    ));
                    if !path_link.is_empty() {
                        *path_clone.lock().unwrap() = path_link;
                    }
                }
                Ok(())
            }),
        );
        let path = path_slot.lock().unwrap().clone();
        let vm_type = type_slot.lock().unwrap().clone();
        (path, vm_type)
    }
}

impl IVmm for Vmm {
    fn assign(&self, machine_id: &str) {
        // Equivalent to Go's signaler listener registered per machine. The
        // listener forwards `creatures/signal` events to the appengine for
        // delivery to the machine's VM instance.
        let trans = Arc::new(VmmListenerCtx {
            app: self.app.clone(),
            storage_root: self.storage_root.clone(),
        });
        let machine_id_owned = machine_id.to_string();
        let listener = Arc::new(Listener {
            id: machine_id.to_string(),
            paused: false,
            dis_time: 0,
            signal: Arc::new(move |key, value| {
                if key != "creatures/signal" {
                    return;
                }
                let raw = serde_json::to_vec(&value).unwrap_or_default();
                let entity_id = serde_json::from_slice::<stores::Send>(&raw)
                    .ok()
                    .map(|p| p.entity_id)
                    .unwrap_or_default();
                let (ast_path, vm_type) =
                    trans.resolve_vm_execution_target(&machine_id_owned, &entity_id);
                let payload = json!({
                    "type": "runVm",
                    "machineId": machine_id_owned,
                    "input": String::from_utf8_lossy(&raw),
                    "astPath": ast_path,
                    "vmType": vm_type,
                });
                let _ = dispatch_packet(&payload);
            }),
        });
        self.app.tools().signaler().listen_to_single(listener);
    }

    fn run_vm(&self, machine_id: &str, store_id: &str, data: &str) {
        // We need `Arc<Self>` to drive the inner helper that touches state.
        // Reconstruct one by leaking a clone of the application interface so
        // method calls dispatch normally; the Arc itself isn't recoverable
        // from `&self`, so this is a thin wrapper.
        let trans = Arc::new(VmmShim {
            app: self.app.clone(),
            storage_root: self.storage_root.clone(),
            storage: self.storage.clone(),
        });
        trans.run_vm(machine_id, store_id, data);
    }

    fn terminate_vm(&self, machine_id: &str) {
        self.send_to_engine(json!({
            "type": "terminateVm",
            "machineId": machine_id,
        }));
    }

    fn build_vm_image(
        &self,
        machine_id: &str,
        entity_id: &str,
        build_path: &str,
        build_type: &str,
    ) {
        self.send_to_engine(json!({
            "type": "buildVmImage",
            "runtime": build_type,
            "machineId": machine_id,
            "entityId": entity_id,
            "imageBuildPath": build_path,
            "buildType": build_type,
        }));
    }

    fn execute_chain_trxs_group(&self, _trxs: Vec<WorkerTrx>) {
        // Go's implementation is also a no-op placeholder ("_ = trxs").
    }

    fn execute_chain_effects(&self, _effects: &str) {
        // Same as Go: no-op placeholder.
    }

    fn close_kvdb(&self) {
        // RocksDB closes via `Drop` on the `Arc<TransactionDB>`; nothing to
        // do here. Matches Go semantics (its closeKvdb body is a stub).
    }

    fn vm_callback(&self, data_raw: &str) -> (String, i64) {
        Vmm::vm_callback(self, data_raw)
    }
}

/// Small shim that owns the same handles as `Vmm` and can be cloned/wrapped
/// in `Arc` so the `&self` IVmm methods can still dispatch through helpers
/// expecting `Arc<Vmm>`.
struct VmmShim {
    app: Arc<dyn ICore>,
    storage_root: String,
    storage: Arc<dyn IStorage>,
}

impl VmmShim {
    fn run_vm(self: &Arc<Self>, machine_id: &str, store_id: &str, data: &str) {
        // Inline of Vmm::run_vm_inner against the shim's handles.
        let store_id_owned = store_id.to_string();
        let machine_id_owned = machine_id.to_string();
        let store_slot = Arc::new(Mutex::new(Store::default()));
        let member_slot = Arc::new(Mutex::new(false));
        let store_clone = store_slot.clone();
        let member_clone = member_slot.clone();
        let store_id_clone = store_id_owned.clone();
        let machine_id_clone = machine_id_owned.clone();
        self.app.modify_state(
            true,
            Box::new(move |trx: &dyn ITrx| {
                let s = Store {
                    id: store_id_clone.clone(),
                    ..Default::default()
                }
                .pull(trx);
                *store_clone.lock().unwrap() = s;
                *member_clone.lock().unwrap() = trx.get_link(&format!(
                    "hasaccess::{}::{}",
                    machine_id_clone, store_id_clone
                )) == "true";
                Ok(())
            }),
        );
        if !*member_slot.lock().unwrap() {
            return;
        }
        let store = store_slot.lock().unwrap().clone();
        let (ast_path, vm_type) =
            resolve_vm_execution_target(&self.app, &self.storage, machine_id, "");
        let send_payload = stores::Send {
            user: User::default(),
            store,
            action: "single".to_string(),
            data: data.to_string(),
            ..Default::default()
        };
        let input = serde_json::to_string(&send_payload).unwrap_or_default();
        let payload = json!({
            "type": "runVm",
            "machineId": machine_id,
            "input": input,
            "astPath": ast_path,
            "vmType": vm_type,
        });
        let _ = dispatch_packet(&payload);
        let _ = &self.storage_root;
    }
}

/// Used by the `assign()` signaler listener — same shape as `VmmShim` but
/// keeps only the handles the listener actually needs.
struct VmmListenerCtx {
    app: Arc<dyn ICore>,
    storage_root: String,
}

impl VmmListenerCtx {
    fn resolve_vm_execution_target(&self, machine_id: &str, entity_id: &str) -> (String, String) {
        let default_path = format!("{}/machines/{}/module", self.storage_root, machine_id);
        resolve_vm_execution_target_inner(&self.app, &default_path, machine_id, entity_id)
    }
}

fn resolve_vm_execution_target(
    app: &Arc<dyn ICore>,
    storage: &Arc<dyn IStorage>,
    machine_id: &str,
    entity_id: &str,
) -> (String, String) {
    let default_path = format!("{}/machines/{}/module", storage.storage_root(), machine_id);
    resolve_vm_execution_target_inner(app, &default_path, machine_id, entity_id)
}

fn resolve_vm_execution_target_inner(
    app: &Arc<dyn ICore>,
    default_path: &str,
    machine_id: &str,
    entity_id: &str,
) -> (String, String) {
    let path_slot = Arc::new(Mutex::new(default_path.to_string()));
    let type_slot = Arc::new(Mutex::new("wasm".to_string()));
    let path_clone = path_slot.clone();
    let type_clone = type_slot.clone();
    let machine_id_owned = machine_id.to_string();
    let entity_id_owned = entity_id.to_string();
    app.modify_state(
        true,
        Box::new(move |trx: &dyn ITrx| {
            let vm = Program {
                machine_id: machine_id_owned.clone(),
                ..Default::default()
            }
            .pull(trx);
            if !vm.path.is_empty() {
                *path_clone.lock().unwrap() = vm.path.clone();
            }
            if !vm.runtime.is_empty() {
                *type_clone.lock().unwrap() = vm.runtime.trim().to_lowercase();
            }
            if !entity_id_owned.is_empty() {
                let runtime_link = trx.get_link(&format!(
                    "vmEntityType::{}::{}",
                    machine_id_owned, entity_id_owned
                ));
                if !runtime_link.is_empty() {
                    *type_clone.lock().unwrap() = runtime_link.trim().to_lowercase();
                }
                let path_link = trx.get_link(&format!(
                    "vmEntityPath::{}::{}",
                    machine_id_owned, entity_id_owned
                ));
                if !path_link.is_empty() {
                    *path_clone.lock().unwrap() = path_link;
                }
            }
            Ok(())
        }),
    );
    let path = path_slot.lock().unwrap().clone();
    let vm_type = type_slot.lock().unwrap().clone();
    (path, vm_type)
}

/// `isManagedRuntime` — runtimes whose VMs run inside the appengine.
pub(super) fn is_managed_runtime(runtime: &str) -> bool {
    let r = runtime.trim().to_lowercase();
    matches!(
        r.as_str(),
        "wasm" | "javascript" | "elpify" | "elpian" | "fire"
    )
}

/// `normalizeRuntime` — Go's `strings.ToLower(TrimSpace(.))`.
pub(super) fn normalize_runtime(runtime: &str) -> String {
    runtime.trim().to_lowercase()
}

/// Field-getter helper — emulates Go's generic `checkField[T]`.
pub(super) fn check_field<'a>(input: &'a Value, key: &str) -> Option<&'a Value> {
    input.get(key)
}

pub(super) fn check_str(input: &Value, key: &str, default: &str) -> String {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| default.to_string())
}

pub(super) fn check_i64(input: &Value, key: &str, default: i64) -> i64 {
    input
        .get(key)
        .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
        .unwrap_or(default)
}

pub(super) fn check_bool(input: &Value, key: &str, default: bool) -> bool {
    if let Some(v) = input.get(key) {
        if let Some(b) = v.as_bool() {
            return b;
        }
        if let Some(s) = v.as_str() {
            return s == "true" || s == "1";
        }
    }
    default
}

/// Convenience for `time.Now().UnixMilli()`.
pub(super) fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[allow(dead_code)]
fn _force_use() -> Result<()> {
    Ok(())
}
