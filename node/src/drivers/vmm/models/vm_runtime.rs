use crate::drivers::vmm::prelude::*;
use crate::drivers::vmm::globals::{GLOBAL_DB, with_global_app};
use crate::drivers::vmm::models::runtime_models::{Trx, WasmDbOp};
use crate::drivers::vmm::bridge::runtime_io::{wasm_send, log, set_log_vm_context};
use crate::drivers::vmm::host::task_graph::host_call;
use crate::models::transaction::ITrx;

pub(crate) static GLOBAL_MANAGED_VMS: Lazy<Arc<Mutex<HashMap<String, ManagedVmHandle>>>> =
    Lazy::new(|| Arc::new(Mutex::new(HashMap::new())));
pub(crate) static GLOBAL_ELPIFY_VMS: Lazy<Arc<Mutex<HashMap<String, Arc<ElpifyManagedVm>>>>> =
    Lazy::new(|| Arc::new(Mutex::new(HashMap::new())));

pub struct WasmMac {
    pub callback: Box<dyn (Fn(JsonValue) -> String) + Send + Sync>,
    pub machine_id: String,
    pub vm_id: String,
    pub store_id: String,
    pub trx: Box<Trx>,
    /// Per-VM JSON transaction held for the full lifecycle.  Lazily initialised
    /// on the first putJson/getJson/getByPrefix/delKey host call and committed
    /// (via ICore) either on an explicit `commitTrx` host call or at VM teardown
    /// inside [`WasmMac::finalize`].
    pub vm_trx: Option<Arc<dyn ITrx>>,
    pub mod_path: String,
    pub cost: u64,
    pub ram_limit_mb: u64,

    pub(crate) execution_result: String,
    pub(crate) has_output: bool,
    pub(crate) stop_: Arc<AtomicBool>,
    pub(crate) running_: Arc<AtomicBool>,
}

pub struct ManagedVmHandle {
    pub(crate) stop: Arc<AtomicBool>,
    pub(crate) running: Arc<AtomicBool>,
}

#[derive(Clone, Debug)]
pub(crate) struct VmResourceLimits {
    pub(crate) max_exec_time_secs: u64,
    pub(crate) ram_mb: u64,
    pub(crate) disk_gb: u64,
    pub(crate) cpu_cores: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum VmRuntime {
    Wasm,
    Elpify,
    Elpian,
    Fire,
}

pub(crate) struct ElpifyTask {
    pub(crate) masm_path: String,
    pub(crate) input_raw: String,
    pub(crate) vm_id: String,
    pub(crate) limits: VmResourceLimits,
}

pub(crate) struct ElpifyManagedVm {
    pub(crate) stop: Arc<AtomicBool>,
    pub(crate) sender: Sender<ElpifyTask>,
}

impl ManagedVmHandle {
    pub fn terminate_vm_instance(&self) {
        self.stop.store(true, Ordering::Relaxed);
        // WasmEdge sync executor in this integration does not expose a hard preemptive kill.
        // Cooperative stop is the available termination mechanism.
    }
}

impl VmResourceLimits {
    pub(crate) fn with_defaults() -> Self {
        VmResourceLimits {
            max_exec_time_secs: 60,
            ram_mb: 64,
            disk_gb: 1,
            cpu_cores: 1,
        }
    }
}

pub(crate) fn parse_vm_resource_limits(packet: &JsonValue) -> VmResourceLimits {
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

/// Length of the collection window each elpify program queue uses to gather
/// transactions before sealing and executing them as one batch.
pub(crate) const ELPIFY_BATCH_WINDOW: Duration = Duration::from_millis(500);

impl ElpifyManagedVm {
    /// Spawn a per-program-entity worker that batches transactions on a 500ms
    /// window and executes each sealed batch sequentially.
    ///
    /// Scheduling contract (per program queue):
    ///   * A fresh window opens the moment the queue starts collecting — i.e.
    ///     when the first transaction of an idle queue arrives, or, back-to-back,
    ///     the instant the previous window closed.
    ///   * The window stays open for [`ELPIFY_BATCH_WINDOW`]; every transaction
    ///     that arrives during it joins the batch.
    ///   * When the window closes the batch is sealed and executed. Execution is
    ///     synchronous on this single worker thread, so two batches of the *same*
    ///     program never overlap (different programs get their own worker, so
    ///     they run concurrently).
    ///   * The queue keeps collecting *during* execution; those transactions form
    ///     the next batch, whose window is anchored to the previous window's
    ///     close. If execution overran that window the next batch fires
    ///     immediately (more than 500ms already elapsed); otherwise it waits out
    ///     the remaining slice of the 500ms period.
    pub(crate) fn new(machine_id: String) -> Self {
        let (tx, rx): (Sender<ElpifyTask>, Receiver<ElpifyTask>) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = Arc::clone(&stop);
        let machine_clone = machine_id.clone();

        thread::spawn(move || {
            // This queue serves one program entity (machine_id + masm_path), so a
            // batch shares one MASM source; cache it across batches anyway.
            let mut source_cache: HashMap<String, String> = HashMap::new();
            let mut pending: Vec<ElpifyTask> = Vec::new();
            // `Some(deadline)` while a collection window is open; `None` when idle.
            let mut next_deadline: Option<Instant> = None;

            'scheduler: loop {
                if stop_clone.load(Ordering::Relaxed) {
                    break;
                }
                match next_deadline {
                    None => {
                        // Idle: block until the first transaction of a new window.
                        // The collection timer starts the moment we start
                        // collecting (now).
                        match rx.recv() {
                            Ok(task) => {
                                pending.push(task);
                                next_deadline = Some(Instant::now() + ELPIFY_BATCH_WINDOW);
                            }
                            Err(_) => break, // every sender dropped → tear down
                        }
                    }
                    Some(deadline) => {
                        let now = Instant::now();
                        if now < deadline {
                            match rx.recv_timeout(deadline - now) {
                                Ok(task) => {
                                    pending.push(task);
                                    continue 'scheduler; // keep collecting
                                }
                                Err(mpsc::RecvTimeoutError::Timeout) => {
                                    // window elapsed → fall through to seal
                                }
                                Err(mpsc::RecvTimeoutError::Disconnected) => {
                                    if !pending.is_empty() {
                                        let batch = std::mem::take(&mut pending);
                                        run_batch_dispatch(
                                            &machine_clone,
                                            &mut source_cache,
                                            &batch,
                                        );
                                    }
                                    break;
                                }
                            }
                        }
                        if stop_clone.load(Ordering::Relaxed) {
                            break;
                        }
                        // Window closed → seal and execute this batch.
                        let batch = std::mem::take(&mut pending);
                        let anchor = deadline; // next window opens back-to-back here
                        if !batch.is_empty() {
                            run_batch_dispatch(&machine_clone, &mut source_cache, &batch);
                        }
                        // The queue kept collecting while the batch ran: pull in
                        // everything that arrived during execution.
                        while let Ok(task) = rx.try_recv() {
                            pending.push(task);
                        }
                        if pending.is_empty() {
                            // Nothing waiting → go idle; a fresh window opens on
                            // the next arriving transaction.
                            next_deadline = None;
                        } else {
                            // Anchor the next window to the previous window's
                            // close. If execution overran it, the next loop sees
                            // `now >= deadline` and fires immediately; otherwise it
                            // waits out the remaining slice of the 500ms period.
                            next_deadline = Some(anchor + ELPIFY_BATCH_WINDOW);
                        }
                    }
                }
            }
        });

        ElpifyManagedVm { stop, sender: tx }
    }

    pub(crate) fn enqueue(&self, task: ElpifyTask) -> Result<(), String> {
        self.sender
            .send(task)
            .map_err(|e| format!("failed to enqueue elpify task: {}", e))
    }

    pub(crate) fn terminate(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Identity of an elpify program *entity*: a distinct MASM program on a machine.
/// Two transactions for the same program key share a queue (sequential batches);
/// different program keys get independent queues (concurrent batches).
pub(crate) fn elpify_program_key(machine_id: &str, masm_path: &str) -> String {
    if masm_path.is_empty() {
        machine_id.to_string()
    } else {
        format!("{}::{}", machine_id, masm_path)
    }
}

/// Enqueue a transaction into the per-program elpify queue, creating the queue
/// (and its batch-scheduler worker) on first use.
pub(crate) fn enqueue_elpify_task(
    machine_id: &str,
    masm_path: String,
    input_raw: String,
    vm_id: String,
    limits: VmResourceLimits,
) -> Result<(), String> {
    let key = elpify_program_key(machine_id, &masm_path);
    let handle = {
        let mut map = GLOBAL_ELPIFY_VMS.lock().unwrap();
        Arc::clone(
            map.entry(key)
                .or_insert_with(|| Arc::new(ElpifyManagedVm::new(machine_id.to_string()))),
        )
    };
    handle.enqueue(ElpifyTask {
        masm_path,
        input_raw,
        vm_id,
        limits,
    })
}

/// Load (and cache) a batch's MASM source, then prove it, surfacing any failure
/// or panic as a `vmOutput` error packet so the host observer is never left in
/// the dark.
fn run_batch_dispatch(
    machine_id: &str,
    source_cache: &mut HashMap<String, String>,
    batch: &[ElpifyTask],
) {
    if batch.is_empty() {
        return;
    }
    let masm_path = batch
        .iter()
        .map(|t| t.masm_path.clone())
        .find(|p| !p.is_empty())
        .unwrap_or_default();
    let vm_id_for_err = batch.last().map(|t| t.vm_id.clone()).unwrap_or_default();

    // Resolve the program source (cached across batches).
    let source = if let Some(s) = source_cache.get(&masm_path) {
        Ok(s.clone())
    } else {
        match std::fs::read_to_string(&masm_path) {
            Ok(s) => {
                source_cache.insert(masm_path.clone(), s.clone());
                Ok(s)
            }
            Err(e) => Err(format!("failed to read MASM file {}: {}", masm_path, e)),
        }
    };

    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let source = source?;
        run_elpify_batch(machine_id, &source, &masm_path, batch)
    }));
    match res {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            log(format!(
                "elpify batch failed for machine {}: {}",
                machine_id, e
            ));
            crate::drivers::vmm::bridge::vm_packet_router::emit_vm_error(
                machine_id,
                &vm_id_for_err,
                "elpify",
                &e,
            );
        }
        Err(payload) => {
            let msg = crate::drivers::vmm::bridge::vm_packet_router::panic_message(&payload);
            log(format!(
                "elpify batch worker panicked for machine {}: {}",
                machine_id, msg
            ));
            crate::drivers::vmm::bridge::vm_packet_router::emit_vm_error(
                machine_id,
                &vm_id_for_err,
                "elpify",
                &format!("panic: {}", msg),
            );
            // The worker thread survives; the next batch is independent.
        }
    }
}

/// Compile a fresh loop-wrapped program for this batch (loop count == number of
/// transactions, each round fed its own payload) and prove it once. Emits a
/// single `vmOutput` packet carrying the batch proof and per-transaction
/// outputs.
fn run_elpify_batch(
    machine_id: &str,
    masm_source: &str,
    masm_path: &str,
    batch: &[ElpifyTask],
) -> Result<(), String> {
    if batch.is_empty() {
        return Ok(());
    }
    // Representative vm context for logging.
    let rep_vm = batch.last().map(|t| t.vm_id.clone()).unwrap_or_default();
    set_log_vm_context(&rep_vm);

    // Per-transaction payloads, in collection order.
    let payloads: Vec<Vec<u64>> = batch
        .iter()
        .map(|t| extract_elpify_inputs(&t.input_raw))
        .collect();

    // Honour the most-recently-supplied resource limits in the batch.
    let limits = batch
        .last()
        .map(|t| t.limits.clone())
        .unwrap_or_else(VmResourceLimits::with_defaults);
    let hard_limit_bytes = limits.ram_mb.saturating_mul(1024).saturating_mul(1024);
    if (masm_source.len() as u64) > hard_limit_bytes {
        return Err(format!(
            "elpify vm exceeded memory limit before execution: source={} bytes limit={} bytes",
            masm_source.len(),
            hard_limit_bytes
        ));
    }

    let started_at = Instant::now();
    let artifacts = execute_batch_with_proof(masm_source, &payloads)
        .map_err(|e| format!("elpify batch execution failed: {}", e))?;
    if started_at.elapsed() > Duration::from_secs(limits.max_exec_time_secs) {
        return Err(format!(
            "elpify vm exceeded max execution time: {} seconds",
            limits.max_exec_time_secs
        ));
    }

    // Encode the STARK proof as base64 (compact for wasm round-trips) and emit a
    // single batch result packet. `outputs` is the per-transaction result vector
    // (capped at BATCH_MAX_EXPOSED_OUTPUTS); a consumer reads its transaction's
    // result by index. The single proof attests to every round.
    let proof_b64 = BASE64_STANDARD.encode(&artifacts.proof_bytes);
    let payloads_json: Vec<JsonValue> = payloads.iter().map(|p| json!(p)).collect();
    let summary = format!(
        "elpify batch executed: {} transaction(s)",
        artifacts.batch_size
    );
    let batch_packet = json!({
        "key": "vmOutput",
        "input": {
            "machineId": machine_id,
            "vmId": rep_vm,
            "runtime": "elpify",
            "logType": "output",
            "masmPath": masm_path,
            "batchSize": artifacts.batch_size,
            "exposedOutputs": artifacts.per_tx_outputs.len(),
            "inputs": payloads_json,
            "outputs": artifacts.per_tx_outputs,
            "stackOutputs": artifacts.stack_outputs,
            "proof": proof_b64,
            "text": summary,
            "data": summary,
        }
    });
    wasm_send(batch_packet);

    log(format!(
        "elpify batch executed machine={} masm={} txs={} outputs={:?}",
        machine_id, masm_path, artifacts.batch_size, artifacts.per_tx_outputs
    ));
    Ok(())
}

pub struct HostData {
    pub(crate) exec: *mut Executor,
    pub(crate) runtime: *mut WasmMac,
}

impl WasmMac {
    pub fn new_vm(
        machine_id: String,
        vm_id: String,
        store_id: String,
        mod_path: String,
        ram_limit_mb: u64,
        cb: Box<dyn (Fn(JsonValue) -> String) + Send + Sync>,
    ) -> Self {
        let stop_ = Arc::new(AtomicBool::new(false));
        let running_ = Arc::new(AtomicBool::new(false));

        WasmMac {
            callback: cb,
            machine_id,
            vm_id,
            store_id,
            trx: Box::new(Trx::new()),
            vm_trx: None,
            mod_path,
            execution_result: "".to_string(),
            has_output: false,
            stop_: stop_,
            running_: running_,
            cost: 0,
            ram_limit_mb: ram_limit_mb.max(1),
        }
    }

    pub fn finalize(&mut self) -> Vec<WasmDbOp> {
        // Auto-commit the per-VM JSON transaction if the VM did not call commitTrx.
        if self.vm_trx.is_some() {
            with_global_app(|app| app.end_vm_trx(&self.vm_id));
            self.vm_trx = None;
        }
        self.trx.commit_as_offchain();
        // Surface the creature's final output (set by the `output` host op) as
        // a runtime vmLog so the host platform can observe the JSON response
        // a wasm program produced for a given signal.
        if self.has_output {
            let payload = serde_json::json!({
                "key": "vmOutput",
                "input": {
                    "text": self.execution_result.clone(),
                    "data": self.execution_result.clone(),
                    "vmId": self.vm_id.clone(),
                    "machineId": self.machine_id.clone(),
                    "logType": "output",
                }
            });
            crate::drivers::vmm::bridge::runtime_io::wasm_send(payload);
        }
        self.trx.ops.clone()
    }

    /// Execute the deployed WASM module against `input`.
    ///
    /// Returns a structured error on any wasmedge failure instead of
    /// panicking. The host-call data (`HostData`) is now bound to a local so
    /// it lives for the entire wasm execution — the previous version passed a
    /// `&mut` to a temporary, which left wasmedge holding a dangling pointer
    /// once the statement ended and caused intermittent segfaults during the
    /// VM tear-down path.
    pub fn execute_on_update(&mut self, input: String) -> Result<(), String> {
        self.running_.store(true, Ordering::Relaxed);
        struct RunningGuard(Arc<AtomicBool>);
        impl Drop for RunningGuard {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Relaxed);
            }
        }
        let _running_guard = RunningGuard(Arc::clone(&self.running_));

        // Declare host_data BEFORE the executor / import module so it
        // outlives every wasmedge handle that holds a raw pointer to it.
        // Rust drops locals in reverse declaration order, so anything that
        // wasmedge owns and may inspect during tear-down (extern_mod, exec,
        // store) will drop before host_data does.
        let mut host_data = HostData {
            exec: std::ptr::null_mut(),
            runtime: self as *mut WasmMac,
        };

        let mut config = Config::create().map_err(|e| format!("wasm config: {}", e))?;
        config.measure_cost(true);
        let bytes = self.ram_limit_mb.saturating_mul(1024).saturating_mul(1024);
        let pages = ((bytes + 65535) / 65536).max(1);
        config.set_max_memory_pages((pages.min(u32::MAX as u64)) as u32);
        let stats = Statistics::create().map_err(|e| format!("wasm stats: {}", e))?;
        let mut store = Store::create().map_err(|e| format!("wasm store: {}", e))?;

        let wasi_mod = wasmedge_sys::WasiModule::create(None, None, None)
            .map_err(|e| format!("wasi module: {}", e))?;

        let mut exec = Executor::create(Some(&config), Some(&stats))
            .map_err(|e| format!("wasm executor: {}", e))?;
        // Now exec exists, finish wiring host_data.
        host_data.exec = &mut exec as *mut Executor;

        let mut dummy: i32 = 1;
        let mut extern_mod = ImportModule::create("env", Box::new(&mut dummy))
            .map_err(|e| format!("env import module: {}", e))?;

        let host_func = unsafe {
            Function::create_sync_func(
                &wasmedge_sys::FuncType::new(vec![ValType::I32, ValType::I32], vec![ValType::I64]),
                host_call,
                &mut host_data,
                1,
            )
            .map_err(|e| format!("hostCall create: {}", e))?
        };
        extern_mod.add_func("hostCall", host_func);

        exec.register_import_module(&mut store, &wasi_mod)
            .map_err(|e| format!("register wasi: {}", e))?;
        exec.register_import_module(&mut store, &extern_mod)
            .map_err(|e| format!("register env: {}", e))?;

        let conf = Config::create().map_err(|e| format!("loader config: {}", e))?;
        let loader = Loader::create(Some(&conf)).map_err(|e| format!("loader: {}", e))?;
        let main_mod_raw = loader
            .from_file(self.mod_path.clone())
            .map_err(|e| format!("load {}: {}", self.mod_path, e))?;
        let conf2 = Config::create().map_err(|e| format!("validator config: {}", e))?;
        let v = Validator::create(Some(&conf2)).map_err(|e| format!("validator: {}", e))?;
        v.validate(&main_mod_raw)
            .map_err(|e| format!("validate {}: {}", self.mod_path, e))?;

        if self.stop_.load(Ordering::Relaxed) {
            return Ok(());
        }
        let mut vm_instance = exec
            .register_active_module(&mut store, &main_mod_raw)
            .map_err(|e| format!("register active module: {}", e))?;

        let mut binding = vm_instance
            .get_func_mut("_start")
            .map_err(|e| format!("missing _start export: {}", e))?;
        exec.call_func(&mut binding, [])
            .map_err(|e| format!("_start call: {}", e))?;

        let val_l = input.len() as i32;
        let mut malloc_fn = vm_instance
            .get_func_mut("malloc")
            .map_err(|e| format!("missing malloc export: {}", e))?;
        let res2 = exec
            .call_func(&mut malloc_fn, [WasmValue::from_i32(val_l)])
            .map_err(|e| format!("malloc call: {}", e))?;
        let val_offset = res2
            .get(0)
            .map(|v| v.to_i32())
            .ok_or_else(|| "malloc returned no value".to_string())?;

        let arr: Vec<u8> = input.as_bytes().to_vec();
        let mut mem = vm_instance
            .get_memory_mut("memory")
            .map_err(|e| format!("get memory: {}", e))?;
        mem.set_data(arr, val_offset.cast_unsigned())
            .map_err(|e| format!("set_data: {}", e))?;

        let c = ((val_offset as i64) << 32) | (val_l as i64);

        if self.stop_.load(Ordering::Relaxed) {
            return Ok(());
        }

        let mut run_fn = vm_instance
            .get_func_mut("run")
            .map_err(|e| format!("missing run export: {}", e))?;
        exec.call_func(&mut run_fn, [WasmValue::from_i64(c)])
            .map_err(|e| format!("run call: {}", e))?;
        Ok(())
    }

    pub fn stop(&mut self) {
        self.stop_.store(true, Ordering::Relaxed);
    }
}

pub(crate) fn detect_vm_runtime(packet: &JsonValue, ast_path: &str) -> VmRuntime {
    let vm_hint = packet["vmType"]
        .as_str()
        .or_else(|| packet["runtime"].as_str())
        .unwrap_or("")
        .to_lowercase();
    if vm_hint == "elpify" || vm_hint == "masm" || ast_path.ends_with(".masm") {
        VmRuntime::Elpify
    } else if vm_hint == "elpian" || vm_hint == "elpian_vm" || ast_path.ends_with(".elpian.json") {
        VmRuntime::Elpian
    } else if vm_hint == "fire" || vm_hint == "firecracker" {
        VmRuntime::Fire
    } else if vm_hint == "javascript" || vm_hint == "quickjs" {
        // quickjs/javascript is currently executed in the managed wasm VM flow.
        VmRuntime::Wasm
    } else {
        VmRuntime::Wasm
    }
}

fn extract_elpify_inputs(input_raw: &str) -> Vec<u64> {
    let parsed: Result<JsonValue, _> = serde_json::from_str(input_raw);
    if parsed.is_err() {
        return vec![];
    }
    let parsed = parsed.unwrap();

    if let Some(arr) = parsed["inputs"].as_array() {
        return arr.iter().filter_map(|v| v.as_u64()).collect();
    }
    if let Some(arr) = parsed["data"]["inputs"].as_array() {
        return arr.iter().filter_map(|v| v.as_u64()).collect();
    }
    if let Some(data_raw) = parsed["data"].as_str() {
        if let Ok(data_json) = serde_json::from_str::<JsonValue>(data_raw) {
            if let Some(arr) = data_json["inputs"].as_array() {
                return arr.iter().filter_map(|v| v.as_u64()).collect();
            }
        }
    }
    vec![]
}

pub(crate) fn parse_u64_array_field(packet: &JsonValue, field_name: &str) -> Vec<u64> {
    packet[field_name]
        .as_array()
        .map(|arr| arr.iter().filter_map(|v| v.as_u64()).collect())
        .unwrap_or_default()
}

pub(crate) fn parse_u8_array_field(packet: &JsonValue, field_name: &str) -> Vec<u8> {
    // Accept either a base64 string (compact — preferred for large blobs like
    // STARK proofs, which are tens of KB and ruinously slow to round-trip as a
    // JSON number array through wasm creatures) or a raw JSON number array.
    match &packet[field_name] {
        JsonValue::String(s) => {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD
                .decode(s.as_bytes())
                .unwrap_or_default()
        }
        JsonValue::Array(arr) => arr
            .iter()
            .filter_map(|v| v.as_u64().and_then(|n| u8::try_from(n).ok()))
            .collect(),
        _ => Vec::new(),
    }
}

pub(crate) fn acquire_resource_lock(resource_id: &str, owner_id: &str) -> Result<(), String> {
    match with_global_app(|app| app.tools().vmm().acquire_resource_lock(resource_id, owner_id)) {
        Some(result) => result,
        None => Err("vmm not initialised".to_string()),
    }
}

pub(crate) fn release_resource_lock(resource_id: &str, owner_id: &str) -> Result<(), String> {
    match with_global_app(|app| app.tools().vmm().release_resource_lock(resource_id, owner_id)) {
        Some(result) => result,
        None => Err("vmm not initialised".to_string()),
    }
}

pub(crate) fn verify_program_execution_from_packet(
    masm_path: &str,
    inputs: &[u64],
    outputs: &[u64],
    proof_bytes: &[u8],
) -> Result<u32, String> {
    if masm_path.is_empty() {
        return Err("masmPath is required".to_string());
    }
    if proof_bytes.is_empty() {
        return Err("proof is required and must be an array of bytes".to_string());
    }

    let artifacts = execute_masm_file_with_proof(masm_path, inputs)
        .map_err(|e| format!("unable to prepare program info for verification: {}", e))?;
    let stack_outputs = stack_outputs_from_ints(outputs)
        .map_err(|e| format!("invalid output values for verification: {}", e))?;

    verify_execution(
        artifacts.program_info,
        artifacts.stack_inputs,
        stack_outputs,
        proof_bytes,
    )
    .map_err(|e| format!("proof verification failed: {}", e))
}


pub(crate) fn execute_elpian_task(
    machine_id: &str,
    vm_id: String,
    ast_path: String,
    input_raw: String,
    limits: VmResourceLimits,
) -> Result<(), String> {
    set_log_vm_context(&vm_id);
    let ast_source = std::fs::read_to_string(&ast_path)
        .map_err(|e| format!("failed to read elpian AST file {}: {}", ast_path, e))?;

    if !elpian_api::create_vm_from_ast(machine_id.to_string(), ast_source) {
        return Err("failed to create elpian VM from AST".to_string());
    }

    let input_json: JsonValue = serde_json::from_str(&input_raw).unwrap_or_else(|_| json!({}));
    let payload = if input_json.get("data").is_some() {
        input_json["data"].clone()
    } else {
        input_json
    };

    let started_at = Instant::now();
    let mut result = elpian_api::execute_vm_func_with_input(
        machine_id.to_string(),
        "main".to_string(),
        payload.to_string(),
        0,
    );
    if let Some(bytes) = elpian_api::vm_memory_usage_bytes(machine_id.to_string()) {
        if bytes > (limits.ram_mb * 1024 * 1024) {
            let _ = elpian_api::destroy_vm(machine_id.to_string());
            return Err(format!(
                "elpian vm exceeded memory limit: used={} bytes limit={} bytes",
                bytes,
                limits.ram_mb * 1024 * 1024
            ));
        }
    }

    while result.has_host_call {
        if started_at.elapsed() > Duration::from_secs(limits.max_exec_time_secs) {
            let _ = elpian_api::destroy_vm(machine_id.to_string());
            return Err(format!(
                "elpian vm exceeded max execution time: {} seconds",
                limits.max_exec_time_secs
            ));
        }
        let call_data: JsonValue = serde_json::from_str(&result.host_call_data)
            .map_err(|e| format!("invalid elpian host call payload: {}", e))?;
        let host_res = json!({"value": wasm_send(call_data)}).to_string();
        result = elpian_api::continue_execution(machine_id.to_string(), host_res);
        if let Some(bytes) = elpian_api::vm_memory_usage_bytes(machine_id.to_string()) {
            if bytes > (limits.ram_mb * 1024 * 1024) {
                let _ = elpian_api::destroy_vm(machine_id.to_string());
                return Err(format!(
                    "elpian vm exceeded memory limit: used={} bytes limit={} bytes",
                    bytes,
                    limits.ram_mb * 1024 * 1024
                ));
            }
        }
    }

    log(format!(
        "elpian vm executed machine={} ast={} result={}",
        machine_id, ast_path, result.result_value
    ));
    let _ = elpian_api::destroy_vm(machine_id.to_string());
    Ok(())
}

pub(crate) fn terminate_managed_vm(machine_id: &str) {
    let mut map = GLOBAL_MANAGED_VMS.lock().unwrap();
    if let Some(handle) = map.remove(machine_id) {
        handle.terminate_vm_instance();
        if handle.running.load(Ordering::Relaxed) {
            log(format!(
                "terminate requested for running vm: {} (cooperative stop signaled)",
                machine_id
            ));
        }
    }
    drop(map);

    // Elpify queues are keyed per program entity (`machine_id` or
    // `machine_id::masm_path`), so terminate every queue belonging to this
    // machine.
    let prefix = format!("{}::", machine_id);
    let mut emap = GLOBAL_ELPIFY_VMS.lock().unwrap();
    let keys: Vec<String> = emap
        .keys()
        .filter(|k| k.as_str() == machine_id || k.starts_with(&prefix))
        .cloned()
        .collect();
    for key in keys {
        if let Some(handle) = emap.remove(&key) {
            handle.terminate();
            log(format!("terminate requested for running elpify vm: {}", key));
        }
    }
}
