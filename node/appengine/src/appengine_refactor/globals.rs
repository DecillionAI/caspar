static RESP_MAP: Lazy<Arc<Mutex<TimedMap<i64, String>>>> =
    Lazy::new(|| Arc::new(Mutex::new(TimedMap::new())));
static TRIGGER_MAP: Lazy<Arc<Mutex<TimedMap<i64, Arc<Condvar>>>>> =
    Lazy::new(|| Arc::new(Mutex::new(TimedMap::new())));
static REQ_ID_COUNTER: Lazy<AtomicI64> = Lazy::new(|| AtomicI64::new(0));
static GLOBAL_REQ_CHAN: Lazy<BlockingQueue<String>> = Lazy::new(|| BlockingQueue::new());
static GLOBAL_HEART_BEAT: Lazy<Arc<Condvar>> = Lazy::new(|| Arc::new(Condvar::new()));
static GLOBAL_MANAGED_VMS: Lazy<Arc<Mutex<HashMap<String, ManagedVmHandle>>>> =
    Lazy::new(|| Arc::new(Mutex::new(HashMap::new())));
static GLOBAL_ELPIFY_VMS: Lazy<Arc<Mutex<HashMap<String, Arc<ElpifyManagedVm>>>>> =
    Lazy::new(|| Arc::new(Mutex::new(HashMap::new())));
static GLOBAL_RESOURCE_LOCKS: Lazy<Arc<Mutex<HashMap<String, Arc<ResourceLockEntry>>>>> =
    Lazy::new(|| Arc::new(Mutex::new(HashMap::new())));
static GLOBAL_DB: Lazy<Arc<Mutex<TransactionDB>>> = Lazy::new(|| {
    let path = "appletdb";
    let mut db_options = Options::default();
    db_options.create_if_missing(true);
    let txn_db_options = TransactionDBOptions::default();
    let db = TransactionDB::open(&db_options, &txn_db_options, path).unwrap();
    Arc::new(Mutex::new(db))
});

struct ResourceLockState {
    locked: bool,
    owner: Option<String>,
    queue: VecDeque<String>,
}

struct ResourceLockEntry {
    state: Mutex<ResourceLockState>,
    cv: Condvar,
}

#[derive(Clone)]
