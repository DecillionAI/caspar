//! The WasmEdge-backed managed VM runtime (`WasmMac`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value as JsonValue};
use wasmedge_sys::{
    config::Config, AsInstance, Compiler, Executor, Function, ImportModule, Instance, Loader,
    Statistics, Store, Validator, WasmValue,
};
use wasmedge_types::ValType;

use caspar_vm_sdk::host::{host, log, set_log_vm_context};
use caspar_vm_sdk::util::{emit_vm_error, panic_message};

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

    /// Execute the deployed WASM module against `input` on a freshly-built
    /// instance (cold path). Retained for the warm pool's fallback and any
    /// one-shot caller; the warm pool reuses `prepare` + `run_in` directly.
    pub fn execute_on_update(&mut self, input: String) -> Result<(), String> {
        self.running_.store(true, Ordering::Relaxed);
        struct RunningGuard(Arc<AtomicBool>);
        impl Drop for RunningGuard {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Relaxed);
            }
        }
        let _running_guard = RunningGuard(Arc::clone(&self.running_));
        let mut prepared = self.prepare()?;
        self.run_in(&mut prepared, &input)
    }

    /// Build a ready-to-run instance: create the WasmEdge store/executor, wire
    /// the `hostCall` import, load the module (a cached AOT artifact when
    /// available), and run `_start` once. The returned [`Prepared`] owns every
    /// handle so it can serve many `run_in` calls — `_start` (the TinyGo runtime
    /// bring-up) is then paid once instead of per signal.
    ///
    /// SAFETY: `Prepared` holds raw pointers into `self` (`HostData.runtime`) and
    /// into its own boxed `Executor`. The caller MUST keep `self` at a stable
    /// address for the whole life of the `Prepared` — the warm actor boxes the
    /// `WasmMac`, and the cold path drops both together at end of scope.
    fn prepare(&mut self) -> Result<Prepared, String> {
        // Guard the FFI boundary: WasmEdge's `Loader::from_file` calls
        // `std::filesystem::absolute(path)` in C++, which THROWS on an empty
        // path — a C++ throw cannot unwind across FFI and aborts the whole node
        // (uncatchable by `catch_unwind`). Never hand WasmEdge an invalid path.
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

        // Boxed so its address is stable while the instance's host function
        // holds a raw pointer to it across reuse.
        let mut host_data: Box<HostData> = Box::new(HostData {
            exec: std::ptr::null_mut(),
            runtime: self as *mut WasmMac,
        });

        let mut config = Config::create().map_err(|e| format!("wasm config: {}", e))?;
        config.measure_cost(true);
        let bytes = self.ram_limit_mb.saturating_mul(1024).saturating_mul(1024);
        let pages = ((bytes + 65535) / 65536).max(1);
        config.set_max_memory_pages((pages.min(u32::MAX as u64)) as u32);
        let stats = Statistics::create().map_err(|e| format!("wasm stats: {}", e))?;
        let mut store = Store::create().map_err(|e| format!("wasm store: {}", e))?;

        let wasi_mod = wasmedge_sys::WasiModule::create(None, None, None)
            .map_err(|e| format!("wasi module: {}", e))?;

        // Boxed so `host_data.exec` (a raw pointer) stays valid across reuse.
        let mut exec: Box<Executor> = Box::new(
            Executor::create(Some(&config), Some(&stats))
                .map_err(|e| format!("wasm executor: {}", e))?,
        );
        host_data.exec = &mut *exec as *mut Executor;

        // The `env` import data is unused (a dummy) — the real host channel is
        // `host_data`, passed to the hostCall function below.
        let mut extern_mod = ImportModule::create("env", Box::new(0i32))
            .map_err(|e| format!("env import module: {}", e))?;

        let host_func = unsafe {
            Function::create_sync_func(
                &wasmedge_sys::FuncType::new(vec![ValType::I32, ValType::I32], vec![ValType::I64]),
                host_call,
                &mut *host_data,
                1,
            )
            .map_err(|e| format!("hostCall create: {}", e))?
        };
        extern_mod.add_func("hostCall", host_func);

        exec.register_import_module(&mut store, &wasi_mod)
            .map_err(|e| format!("register wasi: {}", e))?;
        exec.register_import_module(&mut store, &extern_mod)
            .map_err(|e| format!("register env: {}", e))?;

        // Prefer a cached AOT (native-code) artifact; fall back to the raw
        // `.wasm` (validated) when AOT is unavailable. AOT artifacts were
        // validated when compiled, so re-validating them is waste.
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

        let mut instance = exec
            .register_active_module(&mut store, &main_mod_raw)
            .map_err(|e| format!("register active module: {}", e))?;

        // One-time init: `_start` = TinyGo runtime bring-up (`main` is empty in
        // the endpoint ABI). The AST module can be dropped right after
        // instantiation — WasmEdge copies what it needs into the store.
        {
            let mut start = instance
                .get_func_mut("_start")
                .map_err(|e| format!("missing _start export: {}", e))?;
            exec.call_func(&mut start, [])
                .map_err(|e| format!("_start call: {}", e))?;
        }

        Ok(Prepared {
            _config: config,
            _stats: stats,
            _store: store,
            _wasi: wasi_mod,
            _module: main_mod_raw,
            _extern: extern_mod,
            exec,
            _host_data: host_data,
            instance,
        })
    }

    /// Run one request against an already-`prepare`d instance: copy `input` into
    /// guest memory via `malloc` and invoke the exported `run`. Cheap relative to
    /// `prepare` — no load/validate/instantiate/`_start`.
    fn run_in(&mut self, p: &mut Prepared, input: &str) -> Result<(), String> {
        if self.stop_.load(Ordering::Relaxed) {
            return Ok(());
        }
        let val_l = input.len() as i32;
        let val_offset = {
            let mut malloc_fn = p
                .instance
                .get_func_mut("malloc")
                .map_err(|e| format!("missing malloc export: {}", e))?;
            let res2 = p
                .exec
                .call_func(&mut malloc_fn, [WasmValue::from_i32(val_l)])
                .map_err(|e| format!("malloc call: {}", e))?;
            res2.get(0)
                .map(|v| v.to_i32())
                .ok_or_else(|| "malloc returned no value".to_string())?
        };

        {
            let arr: Vec<u8> = input.as_bytes().to_vec();
            let mut mem = p
                .instance
                .get_memory_mut("memory")
                .map_err(|e| format!("get memory: {}", e))?;
            mem.set_data(arr, val_offset.cast_unsigned())
                .map_err(|e| format!("set_data: {}", e))?;
        }

        let c = ((val_offset as i64) << 32) | (val_l as i64);
        if self.stop_.load(Ordering::Relaxed) {
            return Ok(());
        }
        let mut run_fn = p
            .instance
            .get_func_mut("run")
            .map_err(|e| format!("missing run export: {}", e))?;
        p.exec
            .call_func(&mut run_fn, [WasmValue::from_i64(c)])
            .map_err(|e| format!("run call: {}", e))?;
        Ok(())
    }

    /// Reset per-signal state so a warm instance serves the next request with no
    /// bleed from the previous one.
    fn reset_for_job(&mut self, store_id: String) {
        self.trx = Box::new(Trx::new());
        self.vm_trx_open = false;
        self.store_id = store_id;
        self.execution_result.clear();
        self.has_output = false;
        self.stop_.store(false, Ordering::Relaxed);
    }

    pub fn stop(&mut self) {
        self.stop_.store(true, Ordering::Relaxed);
    }
}

/// A ready-to-run WasmEdge instance whose one-time `_start` init already ran.
/// Owns every handle for the instance's whole life; the boxed `Executor` and
/// `HostData` keep the raw pointers inside the instance's host function valid
/// across reuse. Fields prefixed `_` are kept alive but never read directly.
///
/// Field order is the teardown order (Rust drops fields top-to-bottom) and
/// mirrors the original cold path's reverse-declaration drop sequence: the
/// active `instance` is dropped before the `store` and imports it was built
/// from, and `_host_data` drops last — after `_extern`, whose host function
/// holds a raw pointer into it.
#[allow(dead_code)] // most fields are held only to keep the instance's handles alive
pub(crate) struct Prepared {
    instance: Instance,
    // `ImportModule::create("env", Box::new(0i32))` fixes the data type to i32.
    _extern: ImportModule<i32>,
    exec: Box<Executor>,
    _wasi: wasmedge_sys::WasiModule,
    // The loaded AST module (an `Arc<Module>`, WasmEdge's shared handle) is kept
    // alive for the instance's whole life — the cold path did the same by keeping
    // the local in scope — and dropped after the instance, before the store.
    _module: std::sync::Arc<wasmedge_sys::Module>,
    _store: Store,
    _stats: Statistics,
    _config: Config,
    _host_data: Box<HostData>,
}

// ── Warm instance pool ───────────────────────────────────────────────────────
//
// Rather than build a fresh WasmEdge instance (and re-run the TinyGo `_start`
// init) for every signal, keep one warm instance per machine on its own actor
// thread and feed it signals over a channel. WasmEdge handles are !Send, so the
// instance never leaves its owning thread; per-machine execution is therefore
// serialized — which is exactly what the shared `vm_id="main"` JSON transaction
// already assumes, so this is strictly safer than the old fan-out. An instance
// is recycled after `WARM_MAX_RUNS` (bounds guest memory growth), on any run
// error/panic, and after `WARM_IDLE_SECS` of inactivity. Any failure to build
// falls back to the cold path, so behaviour can never be worse than before.

const WARM_MAX_RUNS: u64 = 500;
const WARM_IDLE_SECS: u64 = 120;
const WARM_POLL_SECS: u64 = 15;

struct WarmJob {
    input: String,
    vm_id: String,
    ast_path: String,
    ram_mb: u64,
    max_exec_secs: u64,
}

struct WarmActor {
    tx: std::sync::mpsc::Sender<WarmJob>,
    stop: Arc<AtomicBool>,
}

fn warm_pool() -> &'static Mutex<HashMap<String, WarmActor>> {
    static CELL: OnceLock<Mutex<HashMap<String, WarmActor>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Route a signal to the machine's warm actor, spawning one on first use.
/// Returns immediately; the VM's response is delivered by `finalize` (the
/// vmOutput packet) exactly as the old per-signal thread did.
pub fn warm_submit(
    machine_id: &str,
    vm_id: &str,
    ast_path: &str,
    input: String,
    ram_mb: u64,
    max_exec_secs: u64,
) {
    let mut job = WarmJob {
        input,
        vm_id: vm_id.to_string(),
        ast_path: ast_path.to_string(),
        ram_mb,
        max_exec_secs,
    };
    let mut pool = warm_pool().lock().unwrap();
    // Clone the handle so the borrow on `pool` ends before we may `remove`.
    let existing = pool.get(machine_id).map(|a| (a.tx.clone(), a.stop.clone()));
    if let Some((tx, stop)) = existing {
        if !stop.load(Ordering::Relaxed) {
            match tx.send(job) {
                Ok(()) => return,
                Err(e) => job = e.0, // actor gone — recover the job and respawn
            }
        }
        pool.remove(machine_id);
    }

    // Spawn a fresh actor for this machine.
    let (tx, rx) = std::sync::mpsc::channel::<WarmJob>();
    let stop = Arc::new(AtomicBool::new(false));
    let running = Arc::new(AtomicBool::new(false));
    // Register in the shared VM table so terminate_managed_vm can stop us.
    global_managed_vms().lock().unwrap().insert(
        machine_id.to_string(),
        ManagedVmHandle {
            stop: stop.clone(),
            running: running.clone(),
        },
    );
    pool.insert(
        machine_id.to_string(),
        WarmActor {
            tx: tx.clone(),
            stop: stop.clone(),
        },
    );
    let mid = machine_id.to_string();
    thread::spawn(move || warm_actor_loop(mid, rx, stop, running));
    let _ = tx.send(job);
}

fn warm_actor_loop(
    machine_id: String,
    rx: std::sync::mpsc::Receiver<WarmJob>,
    stop: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
) {
    use std::sync::mpsc::RecvTimeoutError;
    let mut warm: Option<(Box<WasmMac>, Prepared)> = None;
    let mut current_ast = String::new();
    let mut runs: u64 = 0;
    let mut idle_since = Instant::now();

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let job = match rx.recv_timeout(Duration::from_secs(WARM_POLL_SECS)) {
            Ok(j) => j,
            Err(RecvTimeoutError::Timeout) => {
                // Free the warm instance after inactivity, but keep the actor
                // alive and registered — self-removal here would race a
                // concurrent submit and drop its job. The actor only exits on
                // terminate (stop) or when its channel is fully disconnected.
                if warm.is_some() && idle_since.elapsed() >= Duration::from_secs(WARM_IDLE_SECS) {
                    warm = None;
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        if stop.load(Ordering::Relaxed) {
            break;
        }
        idle_since = Instant::now();
        running.store(true, Ordering::Relaxed);
        set_log_vm_context(&job.vm_id);

        let store_id = serde_json::from_str::<JsonValue>(&job.input)
            .ok()
            .and_then(|v| {
                v.get("store")
                    .and_then(|s| s.get("id"))
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_default();

        let need_build = match &warm {
            None => true,
            Some(_) => current_ast != job.ast_path || runs >= WARM_MAX_RUNS,
        };
        if need_build {
            warm = None; // drop the previous instance first (frees its memory)
            runs = 0;
            let mut mac = Box::new(WasmMac::new_vm(
                machine_id.clone(),
                job.vm_id.clone(),
                store_id.clone(),
                job.ast_path.clone(),
                job.ram_mb,
                make_host_callback(),
            ));
            let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| mac.prepare()));
            match built {
                Ok(Ok(prepared)) => {
                    current_ast = job.ast_path.clone();
                    warm = Some((mac, prepared));
                }
                Ok(Err(e)) => {
                    log(format!(
                        "wasm warm build failed (machine={}, {}); falling back to cold run",
                        machine_id, e
                    ));
                    run_cold(&machine_id, &job, &store_id);
                    running.store(false, Ordering::Relaxed);
                    continue;
                }
                Err(pay) => {
                    log(format!(
                        "wasm warm build panicked (machine={}, {}); falling back to cold run",
                        machine_id,
                        panic_message(&pay)
                    ));
                    run_cold(&machine_id, &job, &store_id);
                    running.store(false, Ordering::Relaxed);
                    continue;
                }
            }
        }

        // Run one job against the warm instance.
        let ran = {
            let entry = warm.as_mut().unwrap();
            let mac = &mut entry.0;
            let prepared = &mut entry.1;
            mac.reset_for_job(store_id);

            // Per-job watchdog: cooperatively abort THIS run on timeout or on a
            // terminate request, without killing the instance for other jobs.
            let done = Arc::new(AtomicBool::new(false));
            {
                let done = done.clone();
                let mac_stop = mac.stop_flag();
                let actor_stop = stop.clone();
                let secs = job.max_exec_secs;
                thread::spawn(move || {
                    let deadline = Instant::now() + Duration::from_secs(secs);
                    while !done.load(Ordering::Relaxed) {
                        if actor_stop.load(Ordering::Relaxed) || Instant::now() >= deadline {
                            mac_stop.store(true, Ordering::Relaxed);
                            return;
                        }
                        thread::sleep(Duration::from_millis(50));
                    }
                });
            }

            let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let r = mac.run_in(prepared, &job.input);
                mac.finalize();
                r
            }));
            done.store(true, Ordering::Relaxed);
            out
        };
        running.store(false, Ordering::Relaxed);

        match ran {
            Ok(Ok(())) => runs += 1,
            Ok(Err(e)) => {
                log(format!("wasm warm vm error (machine={}): {}", machine_id, e));
                emit_vm_error(&machine_id, &job.vm_id, "wasm", &e);
                warm = None; // recycle after an error
            }
            Err(pay) => {
                let msg = panic_message(&pay);
                log(format!(
                    "wasm warm vm panicked (machine={}): {}",
                    machine_id, msg
                ));
                emit_vm_error(&machine_id, &job.vm_id, "wasm", &format!("panic: {}", msg));
                warm = None; // recycle after a panic
            }
        }
    }

    warm = None; // drop the instance before we deregister
    let _ = warm;
    // Deregister only if the registries still point at THIS actor. A terminate
    // followed by a new signal can replace our entry with a fresh actor's before
    // we get here; identity via `Arc::ptr_eq` on our own `stop` prevents us from
    // clobbering that successor (which would leave a duplicate, unregistered VM).
    {
        let mut pool = warm_pool().lock().unwrap();
        if pool
            .get(&machine_id)
            .map(|a| Arc::ptr_eq(&a.stop, &stop))
            .unwrap_or(false)
        {
            pool.remove(&machine_id);
        }
    }
    {
        let mut managed = global_managed_vms().lock().unwrap();
        if managed
            .get(&machine_id)
            .map(|h| Arc::ptr_eq(&h.stop, &stop))
            .unwrap_or(false)
        {
            managed.remove(&machine_id);
        }
    }
}

/// The stateless host dispatch callback every wasm VM uses (reads the global
/// `host()` each call, so it carries no per-VM state and is safe to reuse).
fn make_host_callback() -> Box<dyn (Fn(JsonValue) -> String) + Send + Sync> {
    Box::new(|packet: JsonValue| match host() {
        Some(h) => h.dispatch(&packet),
        None => json!({"ok": false, "error": "caspar vm host is not initialised"}).to_string(),
    })
}

/// One-shot cold execution — the previous per-signal behaviour, used only as the
/// warm pool's fallback when an instance cannot be built. Runs synchronously on
/// the actor thread (the fallback is rare).
fn run_cold(machine_id: &str, job: &WarmJob, store_id: &str) {
    let mut rt = WasmMac::new_vm(
        machine_id.to_string(),
        job.vm_id.clone(),
        store_id.to_string(),
        job.ast_path.clone(),
        job.ram_mb,
        make_host_callback(),
    );
    let stop_flag = rt.stop_flag();
    let secs = job.max_exec_secs;
    let tm = machine_id.to_string();
    let tv = job.vm_id.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(secs));
        if !stop_flag.load(Ordering::Relaxed) {
            stop_flag.store(true, Ordering::Relaxed);
            log(format!(
                "wasm vm timeout reached: machine={} vm={} limit={}s",
                tm, tv, secs
            ));
        }
    });
    let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let r = rt.execute_on_update(job.input.clone());
        rt.finalize();
        r
    }));
    match ran {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            log(format!("wasm vm error: machine={} err={}", machine_id, e));
            emit_vm_error(machine_id, &job.vm_id, "wasm", &e);
        }
        Err(pay) => {
            let msg = panic_message(&pay);
            log(format!(
                "wasm vm panicked: machine={} panic={}",
                machine_id, msg
            ));
            emit_vm_error(machine_id, &job.vm_id, "wasm", &format!("panic: {}", msg));
        }
    }
}
