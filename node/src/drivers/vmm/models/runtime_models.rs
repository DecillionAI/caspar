use crate::drivers::vmm::prelude::*;
use crate::drivers::vmm::globals::{GLOBAL_DB, with_global_app};
use crate::drivers::vmm::bridge::runtime_io::log;

pub struct WasmLock {
    pub mut_: Mutex<()>,
}

impl WasmLock {
    pub fn generate() -> Self {
        WasmLock {
            mut_: Mutex::new(()),
        }
    }
}

pub struct WasmTask {
    pub id: i32,
    pub name: String,
    pub inputs: Arc<Mutex<HashMap<i32, (bool, Arc<Mutex<WasmTask>>)>>>,
    pub outputs: Arc<Mutex<HashMap<i32, Arc<Mutex<WasmTask>>>>>,
    pub vm_index: i32,
    pub started: bool,
}

impl WasmTask {
    pub fn new() -> Self {
        WasmTask {
            id: 0,
            name: String::new(),
            inputs: Arc::new(Mutex::new(HashMap::new())),
            outputs: Arc::new(Mutex::new(HashMap::new())),
            vm_index: 0,
            started: false,
        }
    }
}

pub struct ChainTrx {
    pub machine_id: String,
    pub key: String,
    pub input: String,
    pub user_id: String,
    pub callback_id: String,
}

impl ChainTrx {
    pub fn new(
        machine_id: String,
        key: String,
        input: String,
        user_id: String,
        callback_id: String,
    ) -> Self {
        ChainTrx {
            machine_id,
            key,
            input,
            user_id,
            callback_id,
        }
    }
}

#[derive(Clone)]
pub struct WasmDbOp {
    pub type_: String,
    pub key: String,
    pub val: String,
}

impl WasmDbOp {
    pub fn new() -> Self {
        WasmDbOp {
            type_: String::new(),
            key: String::new(),
            val: String::new(),
        }
    }
}

pub struct Trx {
    pub write_options: WriteOptions,
    pub read_options: ReadOptions,
    pub txn_options: TransactionOptions,
    pub store: BTreeMap<String, String>,
    pub newly_created: BTreeMap<String, bool>,
    pub newly_deleted: BTreeMap<String, bool>,
    pub ops: Vec<WasmDbOp>,
}

impl Trx {
    pub fn new() -> Self {
        Trx {
            write_options: WriteOptions::default(),
            read_options: ReadOptions::default(),
            txn_options: TransactionOptions::default(),
            store: BTreeMap::new(),
            newly_created: BTreeMap::new(),
            newly_deleted: BTreeMap::new(),
            ops: Vec::new(),
        }
    }

    pub fn get_bytes_of_str(&self, str_val: String) -> Vec<u8> {
        let mut bytes: Vec<u8> = str_val.into_bytes();
        bytes.push(0);
        bytes
    }

    pub fn put(&mut self, key: String, val: String) {
        self.ops.push(WasmDbOp {
            type_: "put".to_string(),
            key: key.clone(),
            val: val.clone(),
        });
        self.store.insert(key.clone(), val);
        self.newly_created.insert(key.clone(), true);
        self.newly_deleted.remove(&key);
    }

    pub fn get_by_prefix(&mut self, prefix: String) -> Vec<String> {
        // First collect any locally-buffered values matching the prefix.
        let mut result: Vec<String> = self
            .store
            .iter()
            .filter(|(k, _)| k.starts_with(&prefix))
            .map(|(_, v)| v.clone())
            .collect();

        // Fetch persistent values via ICore (falls back to GLOBAL_DB if the
        // app is not yet initialised, which only happens in unit tests).
        let fetched = if let Some(vals) = with_global_app(|app| {
            let slot = Arc::new(Mutex::new(Vec::<String>::new()));
            let sc   = slot.clone();
            let pfx  = prefix.clone();
            app.modify_state(true, Box::new(move |trx: &dyn crate::models::transaction::ITrx| {
                *sc.lock().unwrap() = trx.get_by_prefix(&pfx);
                Ok(())
            }));
            let v = { slot.lock().unwrap().clone() };
            v
        }) {
            vals
        } else {
            // Fallback: read directly from RocksDB (no ICore available).
            let db = GLOBAL_DB.lock().unwrap();
            let mut raw = Vec::new();
            for t in db.prefix_iterator(prefix.as_bytes()) {
                if let Ok(item) = t {
                    let key = str::from_utf8(&item.0).unwrap_or("").to_string();
                    let val = str::from_utf8(&item.1).unwrap_or("").to_string();
                    if !self.store.contains_key(&key) {
                        raw.push(val);
                    }
                }
            }
            raw
        };
        for v in fetched {
            if !result.contains(&v) {
                result.push(v);
            }
        }
        result
    }

    pub fn get(&mut self, key: String) -> String {
        if let Some(value) = self.store.get(&key) {
            return value.clone();
        }
        if self.newly_deleted.contains_key(&key) {
            return "".to_string();
        }
        // Fetch from ICore; fall back to raw RocksDB if ICore is not available.
        let value = if let Some(v) = with_global_app(|app| {
            let slot   = Arc::new(Mutex::new(String::new()));
            let slot_c = slot.clone();
            let k      = key.clone();
            app.modify_state(true, Box::new(move |trx: &dyn crate::models::transaction::ITrx| {
                *slot_c.lock().unwrap() = trx.get_link(&k);
                Ok(())
            }));
            let v = { slot.lock().unwrap().clone() };
            v
        }) {
            v
        } else {
            let db = GLOBAL_DB.lock().unwrap();
            match db.get(key.as_bytes()) {
                Ok(Some(val)) => str::from_utf8(&val).unwrap_or("").to_string(),
                _ => "".to_string(),
            }
        };
        self.store.insert(key, value.clone());
        value
    }

    pub fn del(&mut self, key: String) {
        let k = String::from(key);
        self.ops.push(WasmDbOp {
            type_: "del".to_string(),
            key: k.clone(),
            val: String::new(),
        });
        self.store.remove(&k);
        self.newly_created.remove(&k);
        self.newly_deleted.insert(k, true);
    }

    /// Commit all buffered ops atomically.
    ///
    /// Uses `ICore::modify_state` (consensus-aware) when available, falling
    /// back to a direct RocksDB transaction only when the app is not yet
    /// initialised (e.g. unit tests).
    pub fn commit_as_offchain(&mut self) {
        if self.ops.is_empty() {
            return;
        }
        let ops = self.ops.clone();
        let committed_via_core = with_global_app(|app| {
            app.modify_state(
                false,
                Box::new(move |trx: &dyn crate::models::transaction::ITrx| {
                    for op in &ops {
                        if op.type_ == "put" {
                            trx.put_link(&op.key, &op.val);
                        } else if op.type_ == "del" {
                            trx.del_key(&op.key);
                        }
                    }
                    Ok(())
                }),
            );
        })
        .is_some();

        if !committed_via_core {
            // Fallback path (no ICore — unit tests / early init).
            let global_db = GLOBAL_DB.lock().unwrap();
            let raw_trx = global_db.transaction();
            for op in &self.ops {
                if op.type_ == "put" {
                    let _ = raw_trx.put(&op.key, &op.val);
                } else if op.type_ == "del" {
                    let _ = raw_trx.delete(&op.key);
                }
            }
            let _ = raw_trx.commit();
        }
        log("committed transaction successfully.".to_string());
    }

    pub fn dummy_commit(&mut self) {}
}

pub struct WasmThreadPool {
    tasks_: Arc<Mutex<VecDeque<Box<dyn FnOnce() + Send>>>>,
    cv_: Arc<Condvar>,
    stop_: Arc<AtomicBool>,
}

impl WasmThreadPool {
    pub fn generate(num_threads: Option<usize>) -> Self {
        let num_threads = num_threads.unwrap_or_else(|| {
            thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
        });
        let vd: VecDeque<Box<dyn FnOnce() + Send>> = VecDeque::new();
        let tasks_: Arc<Mutex<VecDeque<Box<dyn FnOnce() + Send>>>> = Arc::new(Mutex::new(vd));
        let cv_ = Arc::new(Condvar::new());
        let stop_ = Arc::new(AtomicBool::new(false));

        for _i in 0..num_threads {
            let tasks_clone = Arc::clone(&tasks_);
            let cv_clone = Arc::clone(&cv_);
            let stop_clone = Arc::clone(&stop_);

            thread::spawn(move || loop {
                let task = {
                    let tasks = tasks_clone.lock().unwrap();
                    let mut _mg: std::sync::MutexGuard<'_, VecDeque<Box<dyn FnOnce() + Send>>> =
                        cv_clone
                            .wait_while(tasks, |tasks| {
                                tasks.is_empty() && !stop_clone.load(Ordering::Relaxed)
                            })
                            .unwrap();
                    if stop_clone.load(Ordering::Relaxed) && _mg.is_empty() {
                        return;
                    }

                    _mg.pop_front()
                };

                if let Some(task) = task {
                    task();
                }
            });
        }

        WasmThreadPool { tasks_, cv_, stop_ }
    }

    pub fn stop_pool(&mut self) {
        self.stop_.store(true, Ordering::Relaxed);
        self.cv_.notify_all();
    }

    pub fn enqueue<F>(&self, task: F)
    where
        F: FnOnce() + Send + 'static,
    {
        {
            let mut tasks = self.tasks_.lock().unwrap();
            tasks.push_back(Box::new(task));
            drop(tasks);
        }
        self.cv_.notify_one();
    }
}
