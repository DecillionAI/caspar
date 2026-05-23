use crate::drivers::vmm::prelude::*;

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
