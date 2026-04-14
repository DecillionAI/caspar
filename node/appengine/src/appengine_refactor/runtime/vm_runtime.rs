pub struct WasmMac {
    pub callback: Box<dyn (Fn(JsonValue) -> String) + Send + Sync>,
    pub machine_id: String,
    pub vm_id: String,
    pub store_id: String,
    pub trx: Box<Trx>,
    pub mod_path: String,
    pub cost: u64,

    execution_result: String,
    has_output: bool,
    stop_: Arc<AtomicBool>,
    running_: Arc<AtomicBool>,
}

pub struct ManagedVmHandle {
    stop: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum VmRuntime {
    Wasm,
    Elpify,
    Elpian,
    Fire,
}

struct ElpifyTask {
    masm_path: String,
    input_raw: String,
    vm_id: String,
}

struct ElpifyManagedVm {
    stop: Arc<AtomicBool>,
    sender: Sender<ElpifyTask>,
}

impl ManagedVmHandle {
    pub fn terminate_vm_instance(&self) {
        self.stop.store(true, Ordering::Relaxed);
        // WasmEdge sync executor in this integration does not expose a hard preemptive kill.
        // Cooperative stop is the available termination mechanism.
    }
}

impl ElpifyManagedVm {
    fn new(machine_id: String) -> Self {
        let (tx, rx): (Sender<ElpifyTask>, Receiver<ElpifyTask>) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = Arc::clone(&stop);
        let machine_clone = machine_id.clone();

        thread::spawn(move || {
            let engine = ExecutionEngine::new();
            let mut deployed_programs: HashMap<String, u64> = HashMap::new();
            while let Ok(task) = rx.recv() {
                if stop_clone.load(Ordering::Relaxed) {
                    break;
                }
                if let Err(e) = execute_elpify_task(
                    &machine_clone,
                    &engine,
                    &mut deployed_programs,
                    task.masm_path,
                    task.input_raw,
                    task.vm_id,
                ) {
                    log(format!(
                        "elpify task failed for machine {}: {}",
                        machine_clone, e
                    ));
                }
            }
        });

        ElpifyManagedVm { stop, sender: tx }
    }

    fn enqueue(&self, task: ElpifyTask) -> Result<(), String> {
        self.sender
            .send(task)
            .map_err(|e| format!("failed to enqueue elpify task: {}", e))
    }

    fn terminate(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub struct HostData {
    exec: *mut Executor,
    runtime: *mut WasmMac,
}

impl WasmMac {
    pub fn new_vm(
        machine_id: String,
        vm_id: String,
        store_id: String,
        mod_path: String,
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
            mod_path,
            execution_result: "".to_string(),
            has_output: false,
            stop_: stop_,
            running_: running_,
            cost: 0,
        }
    }

    pub fn finalize(&mut self) -> Vec<WasmDbOp> {
        self.trx.commit_as_offchain();
        self.trx.ops.clone()
    }

    pub fn execute_on_update(&mut self, input: String) {
        self.running_.store(true, Ordering::Relaxed);
        struct RunningGuard(Arc<AtomicBool>);
        impl Drop for RunningGuard {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Relaxed);
            }
        }
        let _running_guard = RunningGuard(Arc::clone(&self.running_));

        let mut config = Config::create().unwrap();
        config.measure_cost(true);
        let stats = Statistics::create().unwrap();
        let mut store = Store::create().unwrap();

        let wasi_mod = wasmedge_sys::WasiModule::create(None, None, None).unwrap();

        let mut dummy: i32 = 1;
        let extern_mod = &mut ImportModule::create("env", Box::new(&mut dummy)).unwrap();

        let mut exec = Executor::create(Some(&config), Some(&stats)).unwrap();
        extern_mod.add_func("hostCall", unsafe {
            Function::create_sync_func(
                &wasmedge_sys::FuncType::new(vec![ValType::I32, ValType::I32], vec![ValType::I64]),
                host_call,
                &mut (HostData {
                    exec: &mut exec,
                    runtime: self,
                }),
                1,
            )
            .unwrap()
        });

        exec.register_import_module(&mut store, &wasi_mod).unwrap();
        exec.register_import_module(&mut store, extern_mod).unwrap();

        let conf = Config::create().unwrap();
        let loader = Loader::create(Some(&conf)).unwrap();
        let main_mod_raw = loader.from_file(self.mod_path.clone()).unwrap();
        let conf2 = Config::create().unwrap();
        let v = Validator::create(Some(&conf2)).unwrap();
        v.validate(&main_mod_raw).unwrap();

        let vm_instance_res = exec.register_active_module(&mut store, &main_mod_raw);
        if vm_instance_res.is_ok() {
            if self.stop_.load(Ordering::Relaxed) {
                return;
            }
            let mut vm_instance = vm_instance_res.unwrap();

            let mut binding = vm_instance.get_func_mut("_start").unwrap();

            exec.call_func(&mut binding, []).unwrap();

            let val_l = input.len() as i32;
            let mut malloc_fn = vm_instance.get_func_mut("malloc").unwrap();
            let res2 = exec
                .call_func(&mut malloc_fn, [WasmValue::from_i32(val_l)])
                .unwrap();

            let val_offset = res2[0].to_i32();
            let raw_arr = input.as_bytes();
            let arr: Vec<u8> = raw_arr.to_vec();
            let mem = vm_instance.get_memory_mut("memory");
            mem.unwrap()
                .set_data(arr, val_offset.cast_unsigned())
                .unwrap();
            let c = ((val_offset as i64) << 32) | (val_l as i64);

            if self.stop_.load(Ordering::Relaxed) {
                return;
            }

            let mut run_fn = vm_instance.get_func_mut("run").unwrap();
            let res = exec.call_func(&mut run_fn, [WasmValue::from_i64(c)]);
            if res.is_ok() {
                res.unwrap();
            }
        }
    }

    pub fn stop(&mut self) {
        self.stop_.store(true, Ordering::Relaxed);
    }
}

fn detect_vm_runtime(packet: &JsonValue, ast_path: &str) -> VmRuntime {
    let vm_hint = packet["vmType"].as_str().unwrap_or("").to_lowercase();
    if vm_hint == "elpify" || vm_hint == "masm" || ast_path.ends_with(".masm") {
        VmRuntime::Elpify
    } else if vm_hint == "elpian" || vm_hint == "elpian_vm" || ast_path.ends_with(".elpian.json") {
        VmRuntime::Elpian
    } else if vm_hint == "fire" || vm_hint == "firecracker" {
        VmRuntime::Fire
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

fn parse_u64_array_field(packet: &JsonValue, field_name: &str) -> Vec<u64> {
    packet[field_name]
        .as_array()
        .map(|arr| arr.iter().filter_map(|v| v.as_u64()).collect())
        .unwrap_or_default()
}

fn parse_u8_array_field(packet: &JsonValue, field_name: &str) -> Vec<u8> {
    packet[field_name]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_u64().and_then(|n| u8::try_from(n).ok()))
                .collect()
        })
        .unwrap_or_default()
}

fn get_or_create_resource_lock(resource_id: &str) -> Arc<ResourceLockEntry> {
    let mut map = GLOBAL_RESOURCE_LOCKS.lock().unwrap();
    Arc::clone(map.entry(resource_id.to_string()).or_insert_with(|| {
        Arc::new(ResourceLockEntry {
            state: Mutex::new(ResourceLockState {
                locked: false,
                owner: None,
                queue: VecDeque::new(),
            }),
            cv: Condvar::new(),
        })
    }))
}

fn acquire_resource_lock(resource_id: &str, owner_id: &str) -> Result<(), String> {
    if resource_id.is_empty() {
        return Err("resourceId is required".to_string());
    }
    if owner_id.is_empty() {
        return Err("ownerId is required".to_string());
    }

    let lock = get_or_create_resource_lock(resource_id);
    let mut state = lock.state.lock().unwrap();

    if state.owner.as_deref() == Some(owner_id) {
        return Ok(());
    }
    if !state.queue.iter().any(|x| x == owner_id) {
        state.queue.push_back(owner_id.to_string());
    }

    while state.locked || state.queue.front().map(|x| x.as_str()) != Some(owner_id) {
        state = lock.cv.wait(state).unwrap();
    }

    state.locked = true;
    state.owner = Some(owner_id.to_string());
    state.queue.pop_front();
    Ok(())
}

fn release_resource_lock(resource_id: &str, owner_id: &str) -> Result<(), String> {
    if resource_id.is_empty() {
        return Err("resourceId is required".to_string());
    }
    if owner_id.is_empty() {
        return Err("ownerId is required".to_string());
    }

    let lock = get_or_create_resource_lock(resource_id);
    let mut state = lock.state.lock().unwrap();
    if state.owner.as_deref() != Some(owner_id) {
        return Err("lock owner mismatch".to_string());
    }

    state.locked = false;
    state.owner = None;
    lock.cv.notify_all();
    Ok(())
}

fn verify_program_execution_from_packet(
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

fn execute_elpify_task(
    machine_id: &str,
    engine: &ExecutionEngine,
    deployed_programs: &mut HashMap<String, u64>,
    masm_path: String,
    input_raw: String,
    vm_id: String,
) -> Result<(), String> {
    set_log_vm_context(&vm_id);
    let masm_source = std::fs::read_to_string(&masm_path)
        .map_err(|e| format!("failed to read MASM file {}: {}", masm_path, e))?;

    let program_id = if let Some(program_id) = deployed_programs.get(&masm_path) {
        *program_id
    } else {
        let program_id = engine
            .deploy_program(&masm_source)
            .map_err(|e| format!("failed to deploy MASM in elpify VM: {}", e))?;
        deployed_programs.insert(masm_path.clone(), program_id);
        program_id
    };

    let inputs = extract_elpify_inputs(&input_raw);
    let result = engine
        .submit_task(
            program_id,
            TaskInput {
                inputs: inputs.clone(),
            },
        )
        .map_err(|e| format!("elpify queue execution failed: {}", e))?;

    let output = result
        .runs
        .last()
        .and_then(|r| r.stack_outputs.first())
        .copied()
        .unwrap_or(0);
    log(format!(
        "elpify vm executed machine={} masm={} inputs={:?} output={}",
        machine_id, masm_path, inputs, output
    ));
    Ok(())
}

fn execute_elpian_task(
    machine_id: &str,
    vm_id: String,
    ast_path: String,
    input_raw: String,
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

    let mut result = elpian_api::execute_vm_func_with_input(
        machine_id.to_string(),
        "main".to_string(),
        payload.to_string(),
        0,
    );

    while result.has_host_call {
        let call_data: JsonValue = serde_json::from_str(&result.host_call_data)
            .map_err(|e| format!("invalid elpian host call payload: {}", e))?;
        let host_res = json!({"value": wasm_send(call_data)}).to_string();
        result = elpian_api::continue_execution(machine_id.to_string(), host_res);
    }

    log(format!(
        "elpian vm executed machine={} ast={} result={}",
        machine_id, ast_path, result.result_value
    ));
    let _ = elpian_api::destroy_vm(machine_id.to_string());
    Ok(())
}

fn terminate_managed_vm(machine_id: &str) {
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

    let mut emap = GLOBAL_ELPIFY_VMS.lock().unwrap();
    if let Some(handle) = emap.remove(machine_id) {
        handle.terminate();
        log(format!(
            "terminate requested for running elpify vm: {}",
            machine_id
        ));
    }
}

// Sync task structure
#[derive(Clone)]
