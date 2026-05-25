use crate::drivers::vmm::prelude::*;
use crate::models::core::ICore;

/// Shared core handle published by `Vmm::new` so stateless host-call
/// handlers (e.g. `handle_unified_host_call`) can reach the signaler and
/// other tools without being threaded through every wasmedge callback.
pub(crate) static GLOBAL_APP: Lazy<Mutex<Option<Arc<dyn ICore>>>> =
    Lazy::new(|| Mutex::new(None));

pub(crate) fn set_global_app(app: Arc<dyn ICore>) {
    *GLOBAL_APP.lock().unwrap() = Some(app);
}

pub(crate) fn with_global_app<R, F: FnOnce(&Arc<dyn ICore>) -> R>(f: F) -> Option<R> {
    GLOBAL_APP.lock().unwrap().as_ref().map(f)
}

pub(crate) static GLOBAL_VM_CONTEXT: Lazy<Arc<Mutex<HashMap<String, (String, String)>>>> =
    Lazy::new(|| Arc::new(Mutex::new(HashMap::new())));
pub(crate) static GLOBAL_RESOURCE_LOCKS: Lazy<Arc<Mutex<HashMap<String, Arc<ResourceLockEntry>>>>> =
    Lazy::new(|| Arc::new(Mutex::new(HashMap::new())));
pub(crate) static GLOBAL_DB: Lazy<Arc<Mutex<TransactionDB>>> = Lazy::new(|| {
    let path = "appletdb";
    let mut db_options = Options::default();
    db_options.create_if_missing(true);
    let txn_db_options = TransactionDBOptions::default();
    let db = TransactionDB::open(&db_options, &txn_db_options, path).unwrap();
    Arc::new(Mutex::new(db))
});

pub(crate) struct ResourceLockState {
    pub(crate) locked: bool,
    pub(crate) owner: Option<String>,
    pub(crate) queue: VecDeque<String>,
}

pub(crate) struct ResourceLockEntry {
    pub(crate) state: Mutex<ResourceLockState>,
    pub(crate) cv: Condvar,
}
