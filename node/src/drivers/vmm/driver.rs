//! Translation of `drivers/vmm/vmm.go`.
//!
//! Top-level `Vmm` struct, ZMQ REQ/REP loop, and the high-level public API
//! (`assign`, `run_vm`, `run_vm_entity`, `terminate_vm`, `build_vm_image`,
//! `close_kvdb`). Per-runtime hostcall handlers live in
//! [`hostcall_entities`](super::hostcall_entities) /
//! [`hostcall_logs`](super::hostcall_logs); the dispatcher lives in
//! [`hostcall_global`](super::hostcall_global).

use crate::drivers::vmm::dispatch_packet;
use crate::drivers::vmm::globals::{ResourceLockEntry, ResourceLockState, VmDbBuffer};
use std::sync::Condvar;
use std::collections::VecDeque;
use dashmap::DashMap;
use std::collections::HashMap;
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
///
/// `Vmm` serves as the canonical owner of all per-execution concurrent state.
/// Rather than scattering independent global statics for VM contexts, per-VM
/// transaction buffers, and resource lock registries, they live here as fields
/// so that the entire VMM state has a single, well-defined owner that is
/// accessible through the canonical `ICore → tools() → vmm()` path.
pub struct Vmm {
    pub(super) app: Arc<dyn ICore>,
    pub(super) storage_root: String,
    pub(super) storage: Arc<dyn IStorage>,
    pub(super) file: Arc<dyn IFile>,

    /// vm_id → (creature_id, machine_id): active VM execution context map.
    pub(crate) vm_context: DashMap<String, (String, String)>,

    /// vm_id → write-ahead transaction buffer for Docker/Fire VM executions.
    pub(crate) vm_trx: DashMap<String, Arc<Mutex<VmDbBuffer>>>,

    /// resource_id → per-resource lock state (used by lockResource host call).
    pub(crate) resource_locks: DashMap<String, Arc<ResourceLockEntry>>,

    /// The docker-host bridge gateway. Owned here so docker creatures reach it
    /// only through the canonical `ICore → tools() → vmm()` object graph — there
    /// is no gateway global.
    pub(crate) gateway: Arc<crate::drivers::vmm::network::docker_host::DockerHostGateway>,

    /// docker container name → authoritative VM identity. Populated when the
    /// node launches a docker creature; the gateway resolves a connection's
    /// identity by mapping its source IP to a container name and looking it up
    /// here, so a container can never declare/spoof its own identity.
    pub(crate) vm_containers:
        DashMap<String, crate::drivers::vmm::network::docker_host::ContainerIdentity>,
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
        // Publish the core handle so stateless VM host-call handlers can
        // reach the signaler / storage tools without a Vmm reference.
        crate::drivers::vmm::globals::set_global_app(app.clone());
        // Publish the SDK host bridge and register every VM runtime plugin
        // compiled into this binary (the generated caspar-vm-plugins crate).
        crate::drivers::vmm::host_bridge::init_vm_plugins();
        let gateway = crate::drivers::vmm::network::docker_host::DockerHostGateway::new(app.clone());
        let vmm = Arc::new(Vmm {
            app,
            storage_root: storage_root.to_string(),
            storage,
            file,
            vm_context: DashMap::new(),
            vm_trx: DashMap::new(),
            resource_locks: DashMap::new(),
            gateway,
            vm_containers: DashMap::new(),
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
        let type_slot = Arc::new(Mutex::new(default_runtime_key()));
        let path_clone = path_slot.clone();
        let type_clone = type_slot.clone();
        let machine_id_owned = machine_id.to_string();
        let entity_id_owned = entity_id.to_string();
        self.app.modify_state(
            true,
            Box::new(move |trx: &dyn ITrx| {
                let vm = Program {
                    id: machine_id_owned.clone(),
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

    /// Get or atomically create the resource-lock entry for `resource_id`.
    fn get_or_create_resource_lock(&self, resource_id: &str) -> Arc<ResourceLockEntry> {
        self.resource_locks
            .entry(resource_id.to_string())
            .or_insert_with(|| {
                Arc::new(ResourceLockEntry {
                    state: Mutex::new(ResourceLockState {
                        locked: false,
                        owner: None,
                        queue: std::collections::VecDeque::new(),
                    }),
                    cv: Condvar::new(),
                })
            })
            .clone()
    }

    /// Whether the state mutations of `vm_id` may enter the cluster
    /// consensus: true only for VMs whose program was deployed with
    /// `distribution: "cluster"`. Local-mode VM state never leaves this
    /// instance.
    fn vm_replication_allowed(&self, vm_id: &str) -> bool {
        if !crate::drivers::cluster::is_active() {
            return false;
        }
        let machine_id = self.get_vm_context(vm_id).map(|(_, m)| m);
        let vm_id_owned = vm_id.to_string();
        let slot = Arc::new(Mutex::new(false));
        let slot_clone = slot.clone();
        self.app.modify_state(
            true,
            Box::new(move |trx: &dyn ITrx| {
                let mut distributed =
                    trx.get_link(&format!("vmDistributed::{}", vm_id_owned)) == "true";
                if !distributed {
                    if let Some(m) = &machine_id {
                        distributed =
                            trx.get_link(&format!("vmDistribution::{}", m)) == "cluster";
                    }
                }
                *slot_clone.lock().unwrap() = distributed;
                Ok(())
            }),
        );
        let allowed = *slot.lock().unwrap();
        allowed
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
                // If a docker creature of this machine is connected to the
                // bridge gateway, deliver the signal straight to its container
                // over the live TCP connection instead of cold-spawning a VM.
                // Reached through the canonical tools().vmm() object graph.
                if trans
                    .app
                    .tools()
                    .vmm()
                    .push_signal_to_machine(&machine_id_owned, &key, &value)
                    > 0
                {
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

    // ── Docker-host bridge gateway ────────────────────────────────────────────

    fn start_docker_gateway(&self, port: i64) {
        self.gateway.listen(port);
    }

    fn register_vm_container(
        &self,
        container_name: &str,
        vm_id: &str,
        creature_id: &str,
        program_id: &str,
        machine_id: &str,
    ) {
        self.vm_containers.insert(
            container_name.to_string(),
            crate::drivers::vmm::network::docker_host::ContainerIdentity {
                vm_id: vm_id.to_string(),
                creature_id: creature_id.to_string(),
                program_id: program_id.to_string(),
                machine_id: machine_id.to_string(),
            },
        );
    }

    fn unregister_vm_container(&self, container_name: &str) {
        self.vm_containers.remove(container_name);
    }

    fn identify_container_by_ip(&self, ip: &str) -> Option<(String, String, String, String)> {
        // Ask each registered VM runtime whether it owns a live instance on
        // this source IP (container-style runtimes resolve it through their
        // supervisor), then map the instance name to the identity we recorded
        // at launch.
        let name = caspar_vm_sdk::registry::plugins()
            .into_iter()
            .find_map(|plugin| plugin.identify_instance_by_ip(ip))?;
        self.vm_containers.get(&name).map(|e| {
            let id = e.value();
            (
                id.vm_id.clone(),
                id.creature_id.clone(),
                id.program_id.clone(),
                id.machine_id.clone(),
            )
        })
    }

    fn push_signal_to_machine(&self, machine_id: &str, key: &str, data: &Value) -> usize {
        self.gateway.push_signal_to_machine(machine_id, key, data)
    }

    // ── VM execution context registry ────────────────────────────────────────

    fn register_vm_context(&self, vm_id: &str, creature_id: &str, machine_id: &str) {
        self.vm_context.insert(
            vm_id.to_string(),
            (creature_id.to_string(), machine_id.to_string()),
        );
    }

    fn unregister_vm_context(&self, vm_id: &str) {
        self.vm_context.remove(vm_id);
    }

    fn get_vm_context(&self, vm_id: &str) -> Option<(String, String)> {
        self.vm_context
            .get(vm_id)
            .map(|e| (e.value().0.clone(), e.value().1.clone()))
    }

    // ── Per-VM lifecycle transaction ──────────────────────────────────────────

    fn begin_vm_trx(&self, vm_id: &str) {
        self.vm_trx.insert(
            vm_id.to_string(),
            Arc::new(Mutex::new(VmDbBuffer::new())),
        );
    }

    fn commit_vm_trx(&self, vm_id: &str) {
        if let Some((_, buf_arc)) = self.vm_trx.remove(vm_id) {
            let replicate = self.vm_replication_allowed(vm_id);
            crate::drivers::cluster::with_replication_scope(replicate, || {
                if let Err(e) = buf_arc.lock().unwrap().commit() {
                    eprintln!("[vmm] commit_vm_trx({}) failed: {}", vm_id, e);
                }
            });
        }
    }

    fn vm_db_op(
        &self,
        vm_id: &str,
        op: &str,
        namespaced_key: &str,
        val: &str,
        prefix: &str,
    ) -> Result<String, String> {
        // Look up this VM's lifecycle buffer.
        let buf_arc = if !vm_id.is_empty() {
            self.vm_trx.get(vm_id).map(|r| r.clone())
        } else {
            None
        };

        match op {
            "put" => {
                if let Some(buf) = buf_arc {
                    buf.lock().unwrap().put(namespaced_key.to_string(), val.to_string());
                } else {
                    let k = namespaced_key.to_string();
                    let v = val.to_string();
                    let replicate = self.vm_replication_allowed(vm_id);
                    crate::drivers::cluster::with_replication_scope(replicate, || {
                        self.app.modify_state(
                            false,
                            Box::new(move |trx: &dyn crate::models::transaction::ITrx| {
                                trx.put_link(&k, &v);
                                Ok(())
                            }),
                        );
                    });
                }
                Ok("{}".to_string())
            }
            "get" => {
                // 1. Check write-ahead buffer.
                if let Some(ref buf) = buf_arc {
                    let guard = buf.lock().unwrap();
                    match guard.get_local(namespaced_key) {
                        Some(Some(v)) => return Ok(serde_json::json!({"data": v}).to_string()),
                        Some(None)    => return Ok(serde_json::json!({"data": ""}).to_string()),
                        None          => {}
                    }
                    if let Some(cached) = guard.read_cache.get(namespaced_key) {
                        return Ok(serde_json::json!({"data": cached}).to_string());
                    }
                }
                // 2. Fall through to ICore.
                let k = namespaced_key.to_string();
                let slot = Arc::new(Mutex::new(String::new()));
                let slot_c = slot.clone();
                self.app.modify_state(
                    true,
                    Box::new(move |trx: &dyn crate::models::transaction::ITrx| {
                        *slot_c.lock().unwrap() = trx.get_link(&k);
                        Ok(())
                    }),
                );
                let val_str = { slot.lock().unwrap().clone() };
                if let Some(buf) = buf_arc {
                    buf.lock().unwrap().read_cache.insert(namespaced_key.to_string(), val_str.clone());
                }
                Ok(serde_json::json!({"data": val_str}).to_string())
            }
            "del" => {
                if let Some(buf) = buf_arc {
                    buf.lock().unwrap().del(namespaced_key.to_string());
                } else {
                    let k = namespaced_key.to_string();
                    let replicate = self.vm_replication_allowed(vm_id);
                    crate::drivers::cluster::with_replication_scope(replicate, || {
                        self.app.modify_state(
                            false,
                            Box::new(move |trx: &dyn crate::models::transaction::ITrx| {
                                trx.del_key(&k);
                                Ok(())
                            }),
                        );
                    });
                }
                Ok("{}".to_string())
            }
            "getByPrefix" => {
                let slot = Arc::new(Mutex::new(Vec::<String>::new()));
                let slot_c = slot.clone();
                let pfx = prefix.to_string();
                self.app.modify_state(
                    true,
                    Box::new(move |trx: &dyn crate::models::transaction::ITrx| {
                        *slot_c.lock().unwrap() = trx.get_by_prefix(&pfx);
                        Ok(())
                    }),
                );
                let mut vals = { slot.lock().unwrap().clone() };
                // Overlay write-ahead buffer.
                if let Some(buf) = buf_arc {
                    let guard = buf.lock().unwrap();
                    for (k, v) in &guard.pending_puts {
                        if k.starts_with(prefix) && !vals.contains(v) {
                            vals.push(v.clone());
                        }
                    }
                }
                Ok(serde_json::json!({"data": vals}).to_string())
            }
            _ => Err(format!("unsupported dbOp: {}", op)),
        }
    }

    fn vm_db_commit_explicit(&self, vm_id: &str) -> Result<(), String> {
        if let Some(buf_ref) = self.vm_trx.get(vm_id) {
            let replicate = self.vm_replication_allowed(vm_id);
            crate::drivers::cluster::with_replication_scope(replicate, || {
                buf_ref.lock().unwrap().commit()
            })
        } else {
            Ok(())
        }
    }

    // ── Resource lock management ──────────────────────────────────────────────

    fn acquire_resource_lock(&self, resource_id: &str, owner_id: &str) -> Result<(), String> {
        if resource_id.is_empty() {
            return Err("resourceId is required".to_string());
        }
        if owner_id.is_empty() {
            return Err("ownerId is required".to_string());
        }
        let lock = self.get_or_create_resource_lock(resource_id);
        let mut state = lock.state.lock().unwrap();
        if state.owner.as_deref() == Some(owner_id) {
            return Ok(());
        }
        if state.locked {
            state.queue.push_back(owner_id.to_string());
            loop {
                state = lock.cv.wait(state).unwrap();
                if state.owner.as_deref() == Some(owner_id) {
                    return Ok(());
                }
            }
        }
        state.locked = true;
        state.owner = Some(owner_id.to_string());
        Ok(())
    }

    fn release_resource_lock(&self, resource_id: &str, owner_id: &str) -> Result<(), String> {
        if resource_id.is_empty() {
            return Err("resourceId is required".to_string());
        }
        let lock = match self.resource_locks.get(resource_id) {
            Some(l) => l.clone(),
            None    => return Err(format!("lock '{}' not found", resource_id)),
        };
        let mut state = lock.state.lock().unwrap();
        if state.owner.as_deref() != Some(owner_id) {
            return Err(format!(
                "lock '{}' not owned by '{}'",
                resource_id, owner_id
            ));
        }
        if let Some(next) = state.queue.pop_front() {
            state.owner = Some(next);
        } else {
            state.locked = false;
            state.owner  = None;
        }
        lock.cv.notify_all();
        Ok(())
    }

    // ── Host-call dispatch bridge ─────────────────────────────────────────────

    fn host_action_micro(&self, op: &str, input: &serde_json::Value, req_id: i64) -> (String, i64) {
        self.handle_micro_host_action(op, input, req_id)
    }

    fn host_action_resource_store(&self, op: &str, input: &serde_json::Value, req_id: i64) -> (String, i64) {
        self.handle_resource_store_crud(op, input, req_id)
    }

    fn host_action_resource_entity_create(&self, input: &serde_json::Value, req_id: i64) -> (String, i64) {
        self.handle_resource_entity_create(input, req_id)
    }

    fn host_action_resource_entity_delete(&self, input: &serde_json::Value, req_id: i64) -> (String, i64) {
        self.handle_resource_entity_delete(input, req_id)
    }

    fn host_action_store(&self, op: &str, input: &serde_json::Value, req_id: i64) -> (String, i64) {
        self.handle_store_crud(op, input, req_id)
    }

    fn host_action_creature(&self, op: &str, input: &serde_json::Value, req_id: i64) -> (String, i64) {
        self.handle_creature_crud(op, input, req_id)
    }

    fn host_action_program(&self, op: &str, input: &serde_json::Value, req_id: i64) -> (String, i64) {
        self.handle_program_crud(op, input, req_id)
    }

    // ── Dynamic VM runtime registry (answered by the plugin registry) ─────────

    fn supported_runtimes(&self) -> Vec<String> {
        caspar_vm_sdk::registry::keys()
    }

    fn is_supported_runtime(&self, runtime: &str) -> bool {
        caspar_vm_sdk::registry::is_supported(runtime)
    }

    fn is_managed_runtime(&self, runtime: &str) -> bool {
        caspar_vm_sdk::registry::is_managed(runtime)
    }

    fn runtime_supports_chain_trxs(&self, runtime: &str) -> bool {
        caspar_vm_sdk::registry::supports_chain_trxs(runtime)
    }

    fn runtime_deploy_spec(&self, runtime: &str) -> Option<Value> {
        caspar_vm_sdk::registry::get(runtime).map(|p| p.meta().deploy_spec_json())
    }

    fn plan_run_entity(&self, runtime: &str, ctx: &Value) -> Result<Value, String> {
        caspar_vm_sdk::registry::get(runtime)
            .ok_or_else(|| format!("runtime '{}' is not registered", runtime))?
            .plan_run_entity(ctx)
    }

    fn plan_stop_entity(&self, runtime: &str, ctx: &Value) -> Result<Value, String> {
        caspar_vm_sdk::registry::get(runtime)
            .ok_or_else(|| format!("runtime '{}' is not registered", runtime))?
            .plan_stop_entity(ctx)
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
    let type_slot = Arc::new(Mutex::new(default_runtime_key()));
    let path_clone = path_slot.clone();
    let type_clone = type_slot.clone();
    let machine_id_owned = machine_id.to_string();
    let entity_id_owned = entity_id.to_string();
    app.modify_state(
        true,
        Box::new(move |trx: &dyn ITrx| {
            let vm = Program {
                id: machine_id_owned.clone(),
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

/// `isManagedRuntime` — runtimes whose VMs run inside the node process.
/// Answered dynamically by the VM plugin registry.
pub(super) fn is_managed_runtime(runtime: &str) -> bool {
    caspar_vm_sdk::registry::is_managed(runtime)
}

/// `normalizeRuntime` — Go's `strings.ToLower(TrimSpace(.))`.
pub(super) fn normalize_runtime(runtime: &str) -> String {
    runtime.trim().to_lowercase()
}

/// Canonical key of the registered fallback runtime, used when a program
/// record carries no runtime of its own.
pub(super) fn default_runtime_key() -> String {
    caspar_vm_sdk::registry::default_key().unwrap_or_default()
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
