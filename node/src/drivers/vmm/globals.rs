use crate::drivers::vmm::prelude::*;
use crate::models::core::ICore;
use std::collections::HashSet;

// ── Single global entry point ─────────────────────────────────────────────────
//
// `GLOBAL_APP` is the **only** standalone global in the VMM module.
// Everything else is reachable through: `GLOBAL_APP → tools() → vmm() → method`.
//
// VMM submodules (controllers, host-call handlers) must not introduce their
// own global state — they use `with_global_app` to obtain `&ICore` and then
// traverse the service graph as needed.

pub(crate) static GLOBAL_APP: Lazy<Mutex<Option<Arc<dyn ICore>>>> =
    Lazy::new(|| Mutex::new(None));

pub(crate) fn set_global_app(app: Arc<dyn ICore>) {
    *GLOBAL_APP.lock().unwrap() = Some(app);
}

/// Borrow the global `ICore` handle for the duration of `f`.
///
/// The `GLOBAL_APP` mutex is released **before** `f` is called — the Arc clone
/// is enough to keep the core alive.  This prevents deadlocks when `f` itself
/// needs to acquire other locks.
pub(crate) fn with_global_app<R, F: FnOnce(&Arc<dyn ICore>) -> R>(f: F) -> Option<R> {
    let app = { GLOBAL_APP.lock().unwrap().clone() }?;
    Some(f(&app))
}

// ── Shared data types ─────────────────────────────────────────────────────────
//
// These types are used by the `Vmm` struct fields; they live here so that
// modules which only need the type (not the Vmm impl) can import from one place.

pub(crate) struct ResourceLockState {
    pub(crate) locked: bool,
    pub(crate) owner: Option<String>,
    pub(crate) queue: VecDeque<String>,
}

pub(crate) struct ResourceLockEntry {
    pub(crate) state: Mutex<ResourceLockState>,
    pub(crate) cv: Condvar,
}

/// Per-VM write-ahead transaction buffer.
///
/// One `VmDbBuffer` is created for each Docker/Fire VM execution and stored in
/// `Vmm::vm_trx`.  All `dbOp` writes are buffered here during the VM's
/// lifetime and committed atomically via `ICore::modify_state` when the VM
/// exits (or on an explicit `commitTrx` host call).
///
/// Reads check the buffer first (read-your-own-writes) before falling through
/// to `ICore`.
pub(crate) struct VmDbBuffer {
    pub(crate) pending_puts: HashMap<String, String>,
    pub(crate) pending_dels: HashSet<String>,
    /// Read-through cache to avoid redundant `ICore` round-trips per key.
    pub(crate) read_cache: HashMap<String, String>,
}

impl VmDbBuffer {
    pub(crate) fn new() -> Self {
        VmDbBuffer {
            pending_puts: HashMap::new(),
            pending_dels: HashSet::new(),
            read_cache: HashMap::new(),
        }
    }

    pub(crate) fn put(&mut self, key: String, val: String) {
        self.pending_dels.remove(&key);
        self.read_cache.insert(key.clone(), val.clone());
        self.pending_puts.insert(key, val);
    }

    pub(crate) fn del(&mut self, key: String) {
        self.pending_puts.remove(&key);
        self.read_cache.remove(&key);
        self.pending_dels.insert(key);
    }

    /// Returns `Some(Some(val))` if the key has a pending write,
    /// `Some(None)` if the key has been deleted, `None` if unknown to this buffer.
    pub(crate) fn get_local(&self, key: &str) -> Option<Option<&str>> {
        if self.pending_dels.contains(key) {
            return Some(None);
        }
        if let Some(v) = self.pending_puts.get(key) {
            return Some(Some(v.as_str()));
        }
        None
    }

    /// Flush all buffered writes through `ICore::modify_state` atomically.
    pub(crate) fn commit(&mut self) -> Result<(), String> {
        if self.pending_puts.is_empty() && self.pending_dels.is_empty() {
            return Ok(());
        }
        let puts: Vec<(String, String)> = self.pending_puts.drain().collect();
        let dels: Vec<String> = self.pending_dels.drain().collect();
        let ok_slot = Arc::new(Mutex::new(Ok::<(), String>(())));
        let ok_c = ok_slot.clone();
        with_global_app(|app| {
            app.modify_state(
                false,
                Box::new(move |trx: &dyn crate::models::transaction::ITrx| {
                    for (k, v) in &puts {
                        trx.put_link(k, v);
                    }
                    for k in &dels {
                        trx.del_key(k);
                    }
                    *ok_c.lock().unwrap() = Ok(());
                    Ok(())
                }),
            );
        });
        let result = { ok_slot.lock().unwrap().clone() };
        result
    }
}
