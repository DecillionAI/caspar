use crate::models::worker::Trx;
use serde_json::Value as JsonValue;

/// The virtual machine manager driver interface.
///
/// `IVmm` is the contract through which the rest of the node reaches the VM
/// subsystem.  All VM lifecycle, database, and lock operations are exposed
/// here so that VMM submodules (controllers, host-call handlers) never need
/// their own global statics — they reach everything through the canonical
/// `ICore → tools() → vmm()` path.
pub trait IVmm: Send + Sync {
    // ── VM deployment & execution ─────────────────────────────────────────
    fn assign(&self, machine_id: &str);
    fn run_vm(&self, machine_id: &str, store_id: &str, data: &str);
    fn terminate_vm(&self, machine_id: &str);
    fn build_vm_image(
        &self,
        machine_id: &str,
        entity_id: &str,
        build_path: &str,
        build_type: &str,
    );
    fn execute_chain_trxs_group(&self, trxs: Vec<Trx>);
    fn execute_chain_effects(&self, effects: &str);
    fn close_kvdb(&self);
    /// Returns `(result, gasUsed)`.
    fn vm_callback(&self, data_raw: &str) -> (String, i64);

    // ── Docker-host bridge gateway ───────────────────────────────────────
    /// Start the docker-host bridge gateway TCP listener for docker creatures.
    /// No-op when `port <= 0` or already running. The gateway is owned by the
    /// VMM instance — callers reach it only through `tools().vmm()`.
    fn start_docker_gateway(&self, port: i64);

    /// Start the VMM HTTP ingress listener. It accepts requests shaped as
    /// `/{creatureId}/{programId}/{entityId}/{vmId}/{path…}` and forwards them
    /// to the HTTP server of the named VM instance (docker proxies to the
    /// container; other runtimes fall back to signalling). No-op when
    /// `port <= 0` or already running.
    fn start_http_ingress(&self, port: i64);
    /// Bind a docker container *name* to its VM's authoritative identity. Done
    /// when the node launches the container, so a gateway connection can be
    /// identified by resolving its source IP → container name → this identity.
    fn register_vm_container(
        &self,
        container_name: &str,
        vm_id: &str,
        creature_id: &str,
        program_id: &str,
        machine_id: &str,
    );
    /// Drop a container→identity binding at VM teardown.
    fn unregister_vm_container(&self, container_name: &str);
    /// Identify the creature/VM a connection belongs to from its docker-network
    /// source `ip`: the node asks docker which container owns that IP, then maps
    /// the container name to its registered identity. Returns
    /// `(vm_id, creature_id, program_id, machine_id)`. Spoof-resistant — the
    /// container cannot forge its bridge IP or docker's view of it.
    fn identify_container_by_ip(&self, ip: &str) -> Option<(String, String, String, String)>;
    /// Push a signal to every live docker container of `machine_id`, delivered
    /// over its gateway connection. Returns the number of containers reached.
    fn push_signal_to_machine(&self, machine_id: &str, key: &str, data: &JsonValue) -> usize;

    // ── VM execution context registry ────────────────────────────────────
    /// Register an active VM execution context (vm_id → creature/machine).
    fn register_vm_context(&self, vm_id: &str, creature_id: &str, machine_id: &str);
    /// Remove a VM execution context when the VM terminates.
    fn unregister_vm_context(&self, vm_id: &str);
    /// Look up the (creature_id, machine_id) for a running VM.
    fn get_vm_context(&self, vm_id: &str) -> Option<(String, String)>;

    // ── Per-VM lifecycle transaction management ──────────────────────────
    /// Open a write-ahead transaction buffer for `vm_id`.
    /// All subsequent `vm_db_op` writes for this VM are buffered until commit.
    fn begin_vm_trx(&self, vm_id: &str);
    /// Commit all buffered writes for `vm_id` atomically via `ICore::modify_state`,
    /// then discard the buffer.
    fn commit_vm_trx(&self, vm_id: &str);
    /// Execute a key-value DB operation for a running VM, routing through the
    /// VM's write-ahead buffer when one exists.
    ///
    /// `op`: `"put"` | `"get"` | `"del"` | `"getByPrefix"`.
    /// `namespaced_key`: fully-qualified storage key (e.g. `AppletDb::...`).
    /// `val`: value for `"put"` ops (ignored otherwise).
    /// `prefix`: namespace prefix for `"getByPrefix"` ops (ignored otherwise).
    fn vm_db_op(
        &self,
        vm_id: &str,
        op: &str,
        namespaced_key: &str,
        val: &str,
        prefix: &str,
    ) -> Result<String, String>;
    /// Flush the VM's current buffer immediately (mid-execution explicit commit).
    fn vm_db_commit_explicit(&self, vm_id: &str) -> Result<(), String>;

    // ── Resource lock management ─────────────────────────────────────────
    /// Acquire an exclusive lock on `resource_id` for `owner_id`.
    /// Blocks until the lock is available (FIFO queue).
    fn acquire_resource_lock(&self, resource_id: &str, owner_id: &str) -> Result<(), String>;
    /// Release the lock on `resource_id` held by `owner_id`.
    fn release_resource_lock(&self, resource_id: &str, owner_id: &str) -> Result<(), String>;

    // ── Host-call dispatch bridge ────────────────────────────────────────
    /// Dispatch a micro host action (genId, getLink, putJson, …).
    fn host_action_micro(&self, op: &str, input: &JsonValue, req_id: i64) -> (String, i64);
    /// Dispatch a resource-store CRUD host action.
    fn host_action_resource_store(&self, op: &str, input: &JsonValue, req_id: i64) -> (String, i64);
    /// Dispatch a resource-entity create/delete host action.
    fn host_action_resource_entity_create(&self, input: &JsonValue, req_id: i64) -> (String, i64);
    fn host_action_resource_entity_delete(&self, input: &JsonValue, req_id: i64) -> (String, i64);
    /// Dispatch a store CRUD host action.
    fn host_action_store(&self, op: &str, input: &JsonValue, req_id: i64) -> (String, i64);
    /// Dispatch a creature CRUD host action.
    fn host_action_creature(&self, op: &str, input: &JsonValue, req_id: i64) -> (String, i64);
    /// Dispatch a program CRUD host action (get/list/update/listByMachine).
    fn host_action_program(&self, op: &str, input: &JsonValue, req_id: i64) -> (String, i64);

    // ── Dynamic VM runtime registry ──────────────────────────────────────
    //
    // The node never hardcodes VM type keys; everything below is answered by
    // the VM plugin registry, so the set of supported runtimes is exactly the
    // set of plugins the host admin compiled into this binary.

    /// Canonical keys of every VM runtime compiled into this node.
    fn supported_runtimes(&self) -> Vec<String>;
    /// Whether `runtime` (key or alias) names a registered VM type.
    fn is_supported_runtime(&self, runtime: &str) -> bool;
    /// Whether `runtime` executes inside the node process (managed runtime).
    fn is_managed_runtime(&self, runtime: &str) -> bool;
    /// Whether `runtime` executes grouped chain transactions.
    fn runtime_supports_chain_trxs(&self, runtime: &str) -> bool;
    /// Deploy behaviour of `runtime` as declared by its plugin:
    /// `{ entityFileName, acceptsExtraFiles, buildOnDeploy,
    ///    setEntityLinksOnDeploy }`. `None` for unknown runtimes.
    fn runtime_deploy_spec(&self, runtime: &str) -> Option<JsonValue>;
    /// Ask `runtime`'s plugin to plan a standalone entity launch.
    /// `ctx`: `{ machineId, programId, entityId, vmId, resources, params }` →
    /// `{ input, links: [[key, value], ...] }`.
    fn plan_run_entity(&self, runtime: &str, ctx: &JsonValue) -> Result<JsonValue, String>;
    /// Ask `runtime`'s plugin to plan a standalone entity stop.
    /// `ctx`: `{ machineId, programId, entityId, vmId }` →
    /// `{ input, links: [{field, key, required}, ...] }`.
    fn plan_stop_entity(&self, runtime: &str, ctx: &JsonValue) -> Result<JsonValue, String>;

    /// Forward a packaged inbound HTTP request to the VM instance it targets.
    ///
    /// `request`: `{ creatureId, programId, entityId, vmId, method, path,
    ///              query, headers, bodyBase64 }`.
    ///
    /// The VMM resolves the entity's module path + runtime (the same
    /// resolution the signal listener and `runVm` use) and dispatches a
    /// `forwardHttp` packet to the entity's plugin through the packet router —
    /// docker proxies to the container's HTTP server, every other runtime
    /// falls back to signalling the VM. Returns the plugin's response value
    /// `{ ok, status, headers, body | bodyBase64 }`. Owning this here keeps
    /// forwarding a capability of the VMM instance rather than free-standing
    /// logic reaching into the packet router directly.
    fn forward_http(&self, request: &JsonValue) -> JsonValue;
}
