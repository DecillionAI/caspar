//! The WasmEdge-backed managed VM runtime (`WasmMac`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::Value as JsonValue;
use wasmedge_sys::{
    config::Config, AsInstance, Compiler, Executor, Function, ImportModule, Loader, Statistics,
    Store, Validator, WasmValue,
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

// ── AOT module cache ─────────────────────────────────────────────────────────
//
// Every signal used to load the raw `.wasm` off disk, re-validate the whole
// module, and run it in the interpreter — paid in full on each call. Instead we
// compile each deployed module to a WasmEdge *universal wasm* (native code in a
// custom section) exactly once, cache it on disk next to the source keyed by the
// source's mtime, and load that afterwards. The universal wasm loads through the
// ordinary `Loader::from_file` path and exposes the identical exports, so nothing
// else in `execute_on_update` changes; it just runs as native code and skips
// re-validation. Any failure (a WasmEdge built without LLVM, a read-only cache
// dir, a compiler panic) latches AOT off and falls back to the interpreter, so
// behaviour degrades to exactly what it was before.

/// Latched once an AOT compile fails, so a host that cannot compile never pays a
/// doomed compile again and quietly stays on the interpreter path.
fn aot_unavailable() -> &'static AtomicBool {
    static CELL: OnceLock<AtomicBool> = OnceLock::new();
    CELL.get_or_init(|| AtomicBool::new(false))
}

/// Per-module compile lock so concurrent signals to a cold module compile it
/// once instead of racing to write the same cache file.
fn aot_locks() -> &'static Mutex<HashMap<String, Arc<Mutex<()>>>> {
    static CELL: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Global gate serializing ALL AOT compilations across modules.
///
/// WasmEdge's AOT backend lowers each module to intermediate object files whose
/// functions are named generically (`f0`, `f1`, … — one set per module). When
/// two *different* modules compile concurrently, those object files collide at
/// the `ld.lld` link step (`error: duplicate symbol: f10`), the compile fails,
/// and AOT latches off for the rest of the process. The per-module `aot_locks`
/// only dedupe compiles of the *same* module, so cross-module compilation must
/// be serialized here. AOT is one-time per module (the result is cached to
/// disk), so this only serializes first-time warm-up and never touches the hot
/// path where a cached artifact already exists.
fn aot_compile_gate() -> &'static Mutex<()> {
    static CELL: OnceLock<Mutex<()>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(()))
}

/// AOT is on unless `CASPAR_WASM_AOT` is explicitly a falsey value.
fn aot_enabled() -> bool {
    match std::env::var("CASPAR_WASM_AOT") {
        Ok(v) => {
            let v = v.trim().to_ascii_lowercase();
            !(v == "0" || v == "false" || v == "off" || v == "no")
        }
        Err(_) => true,
    }
}

fn file_mtime(path: &str) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Resolve the module file to actually load: a fresh cached AOT artifact when one
/// is available (`(path, true)`), otherwise the original `.wasm` (`(path, false)`,
/// which the caller still validates).
fn resolve_module_artifact(mod_path: &str) -> (String, bool) {
    if !aot_enabled() || aot_unavailable().load(Ordering::Relaxed) {
        return (mod_path.to_string(), false);
    }
    let src_mtime = match file_mtime(mod_path) {
        Some(t) => t,
        None => return (mod_path.to_string(), false),
    };
    let cache_path = format!("{}.aot", mod_path);
    // Fast path: a cache at least as new as the source already exists.
    if let Some(cache_mtime) = file_mtime(&cache_path) {
        if cache_mtime >= src_mtime {
            return (cache_path, true);
        }
    }
    // Serialize compilation of this specific module across concurrent signals.
    let lock = {
        let mut map = aot_locks().lock().unwrap();
        map.entry(mod_path.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    };
    let _guard = lock.lock().unwrap();
    // Re-check under the lock — another thread may have just built it, or another
    // module may have latched AOT off while we waited.
    if let Some(cache_mtime) = file_mtime(&cache_path) {
        if cache_mtime >= src_mtime {
            return (cache_path, true);
        }
    }
    if aot_unavailable().load(Ordering::Relaxed) {
        return (mod_path.to_string(), false);
    }
    match compile_aot(mod_path, &cache_path) {
        Ok(()) => {
            log(format!("wasm AOT compiled: {} -> {}", mod_path, cache_path));
            (cache_path, true)
        }
        Err(e) => {
            aot_unavailable().store(true, Ordering::Relaxed);
            log(format!(
                "wasm AOT unavailable ({}); using interpreter for {} and later modules",
                e, mod_path
            ));
            (mod_path.to_string(), false)
        }
    }
}

/// Compile `src` (`.wasm`) to a cached universal-wasm at `dst`, atomically. The
/// WasmEdge FFI is wrapped in `catch_unwind` so a host without an AOT-capable
/// WasmEdge reports a normal error (→ interpreter fallback) instead of aborting.
fn compile_aot(src: &str, dst: &str) -> Result<(), String> {
    // Compile to a temp file then rename, so a crash mid-compile never leaves a
    // half-written artifact a later signal would try to load.
    let tmp = format!("{}.tmp.{}", dst, std::process::id());
    let src_owned = src.to_string();
    let tmp_for_compile = tmp.clone();
    // Serialize the compile itself across all modules (see `aot_compile_gate`).
    // Held only for the duration of the LLVM/link work; recovered from a
    // poisoned lock so a single failed compile can never wedge AOT for the
    // whole process (the panic is contained by the `catch_unwind` below, so in
    // practice the guard is dropped cleanly).
    let _gate = aot_compile_gate()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<(), String> {
        // `Wasm` output format = universal wasm (native code in a custom section),
        // so the artifact loads through the normal Loader path and stays portable
        // to a plain interpreter if ever copied to a non-AOT host.
        let mut config = Config::create().map_err(|e| format!("aot config: {}", e))?;
        config.set_aot_compiler_output_format(wasmedge_types::CompilerOutputFormat::Wasm);
        let compiler =
            Compiler::create(Some(&config)).map_err(|e| format!("aot create: {}", e))?;
        compiler
            .compile_from_file(&src_owned, &tmp_for_compile)
            .map_err(|e| format!("aot compile: {}", e))?;
        Ok(())
    }));
    let compiled = match res {
        Ok(inner) => inner,
        Err(_) => Err("aot compiler panicked".to_string()),
    };
    match compiled {
        Ok(()) => std::fs::rename(&tmp, dst).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("aot cache rename: {}", e)
        }),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
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

        // Prefer a cached AOT (native-code) artifact for this module; fall back
        // to the raw `.wasm` when AOT is unavailable. `is_aot` artifacts were
        // validated when compiled, so re-validating them every signal is waste.
        let (load_path, is_aot) = resolve_module_artifact(&mod_path);

        let conf = Config::create().map_err(|e| format!("loader config: {}", e))?;
        let loader = Loader::create(Some(&conf)).map_err(|e| format!("loader: {}", e))?;
        let main_mod_raw = loader
            .from_file(&load_path)
            .map_err(|e| format!("load {}: {}", load_path, e))?;
        if !is_aot {
            let conf2 = Config::create().map_err(|e| format!("validator config: {}", e))?;
            let v = Validator::create(Some(&conf2)).map_err(|e| format!("validator: {}", e))?;
            v.validate(&main_mod_raw)
                .map_err(|e| format!("validate {}: {}", load_path, e))?;
        }

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
