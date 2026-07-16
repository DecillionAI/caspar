//! The WasmEdge-backed managed VM runtime (`WasmMac`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::Value as JsonValue;
use wasmedge_sys::{
    config::Config, AsInstance, Executor, Function, ImportModule, Loader, Statistics, Store,
    Validator, WasmValue,
};
use wasmedge_types::ValType;

use caspar_vm_sdk::host::{host, log};

use crate::host_calls::host_call;
use crate::models::Trx;

pub(crate) fn global_managed_vms() -> &'static Arc<Mutex<HashMap<String, ManagedVmHandle>>> {
    static CELL: OnceLock<Arc<Mutex<HashMap<String, ManagedVmHandle>>>> = OnceLock::new();
    CELL.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

pub struct ManagedVmHandle {
    pub(crate) stop: Arc<AtomicBool>,
    pub(crate) running: Arc<AtomicBool>,
}

impl ManagedVmHandle {
    pub fn terminate_vm_instance(&self) {
        self.stop.store(true, Ordering::Relaxed);
        // WasmEdge's sync executor in this integration does not expose a hard
        // preemptive kill; cooperative stop is the termination mechanism.
    }
}

/// Signal a cooperative stop to the managed wasm VM of `machine_id`.
pub fn terminate_managed_vm(machine_id: &str) {
    let mut map = global_managed_vms().lock().unwrap();
    if let Some(handle) = map.remove(machine_id) {
        handle.terminate_vm_instance();
        if handle.running.load(Ordering::Relaxed) {
            log(format!(
                "terminate requested for running vm: {} (cooperative stop signaled)",
                machine_id
            ));
        }
    }
}

pub struct WasmMac {
    pub callback: Box<dyn (Fn(JsonValue) -> String) + Send + Sync>,
    pub machine_id: String,
    pub vm_id: String,
    pub store_id: String,
    pub trx: Box<Trx>,
    /// Whether this VM opened a per-lifecycle JSON transaction on the host.
    /// Lazily set on the first putJson/getJson/getByPrefix/delKey host call
    /// and closed (committing) either on an explicit `commitTrx` host call or
    /// at VM teardown inside [`WasmMac::finalize`].
    pub vm_trx_open: bool,
    pub mod_path: String,
    pub cost: u64,
    pub ram_limit_mb: u64,

    pub(crate) execution_result: String,
    pub(crate) has_output: bool,
    pub(crate) stop_: Arc<AtomicBool>,
    pub(crate) running_: Arc<AtomicBool>,
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
        WasmMac {
            callback: cb,
            machine_id,
            vm_id,
            store_id,
            trx: Box::new(Trx::new()),
            vm_trx_open: false,
            mod_path,
            execution_result: "".to_string(),
            has_output: false,
            stop_: Arc::new(AtomicBool::new(false)),
            running_: Arc::new(AtomicBool::new(false)),
            cost: 0,
            ram_limit_mb: ram_limit_mb.max(1),
        }
    }

    pub(crate) fn stop_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.stop_)
    }

    pub(crate) fn running_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.running_)
    }

    pub fn finalize(&mut self) {
        // Auto-commit the per-VM JSON transaction if the VM did not call
        // commitTrx.
        if self.vm_trx_open {
            if let Some(h) = host() {
                h.end_vm_json_trx(&self.vm_id);
            }
            self.vm_trx_open = false;
        }
        self.trx.commit_as_offchain();
        // Surface the creature's final output (set by the `output` host op) as
        // a runtime vmOutput packet so the host platform can observe the JSON
        // response a wasm program produced for a given signal.
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
            if let Some(h) = host() {
                let _ = h.dispatch(&payload);
            }
        }
    }

    /// Execute the deployed WASM module against `input`.
    ///
    /// Returns a structured error on any wasmedge failure instead of
    /// panicking. The host-call data (`HostData`) is bound to a local so it
    /// lives for the entire wasm execution — passing a `&mut` to a temporary
    /// left wasmedge holding a dangling pointer once the statement ended and
    /// caused intermittent segfaults during VM tear-down.
    pub fn execute_on_update(&mut self, input: String) -> Result<(), String> {
        // Guard the FFI boundary: WasmEdge's `Loader::from_file` calls
        // `std::filesystem::absolute(path)` in C++, which THROWS on an empty
        // path ("cannot make absolute path: Invalid argument"). A C++ throw
        // cannot unwind across the FFI boundary, so it aborts the whole node
        // process (`std::terminate`) — uncatchable by the controller's
        // `catch_unwind`. We must therefore never hand WasmEdge an invalid
        // module path: validate it here and surface a normal Result::Err (which
        // the controller turns into a contained vmOutput error) instead.
        let mod_path = self.mod_path.trim().to_string();
        if mod_path.is_empty() {
            return Err(
                "wasm module path is empty (no astPath resolved for this entity); \
                 refusing to load"
                    .to_string(),
            );
        }
        if !std::path::Path::new(&mod_path).is_file() {
            return Err(format!("wasm module file not found: {}", mod_path));
        }

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
            .from_file(mod_path.clone())
            .map_err(|e| format!("load {}: {}", mod_path, e))?;
        let conf2 = Config::create().map_err(|e| format!("validator config: {}", e))?;
        let v = Validator::create(Some(&conf2)).map_err(|e| format!("validator: {}", e))?;
        v.validate(&main_mod_raw)
            .map_err(|e| format!("validate {}: {}", mod_path, e))?;

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
