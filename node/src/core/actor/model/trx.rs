//! Translation of `core/module/actor/model/trx/trx.go`.
//!
//! `TrxWrapper` implements [`ITrx`] on top of the RocksDB key/value store.
//!
//! Go's original used Badger's `*badger.Txn` for MVCC isolation. Rust's
//! `rocksdb::Transaction` borrows from its `TransactionDB` via a lifetime,
//! which doesn't compose with the long-lived per-call transaction object the
//! `ITrx` trait demands. The translation therefore models the transaction as
//! a write-back overlay over the underlying DB:
//!
//!   * Reads consult the in-memory `overlay` first, then fall back to the DB.
//!   * Writes / deletes update the overlay and append an `Update` entry to
//!     `changes` (same shape as Go's `tw.Changes`).
//!   * `commit()` flushes the overlay through a single RocksDB `WriteBatch`,
//!     producing atomicity comparable to a transaction commit.
//!   * `discard()` simply drops the overlay (and `changes`).
//!
//! This matches the externally-visible semantics of the Go wrapper (callers
//! see their own writes inside the same `ModifyState` block; concurrent
//! callers in `core.ModifyState` already serialise via the higher-level
//! Mutex) without the lifetime gymnastics required to embed a real RocksDB
//! transaction in a self-referential struct.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use rocksdb::{IteratorMode, WriteBatchWithTransaction};
use rsa::{RsaPrivateKey, RsaPublicKey};
use serde_json::{Map, Value};

use crate::models::ports::storage::IStorage;
use crate::models::core::ICore;
use crate::models::transaction::ITrx;
use crate::models::update::Update;
use crate::shell::utils::crypto as cryp;

/// `TrxWrapper` is the per-call transaction handle.
pub struct TrxWrapper {
    _core: Arc<dyn ICore>,
    db: crate::models::ports::storage::KvDb,
    readonly: bool,
    inner: Mutex<Inner>,
}

struct Inner {
    /// Overlay: `Some(value)` for puts, `None` for tombstones.
    overlay: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
    /// Update log (mirrors Go's `Changes`).
    changes: Vec<Update>,
    /// Mark the wrapper as already committed/discarded so a second call is
    /// a no-op rather than re-applying writes.
    finalized: bool,
}

impl TrxWrapper {
    /// `NewTrx(core, storage, readonly)`.
    pub fn new(
        core: Arc<dyn ICore>,
        storage: Arc<dyn IStorage>,
        readonly: bool,
    ) -> Arc<TrxWrapper> {
        Arc::new(TrxWrapper {
            _core: core,
            db: storage.kv_db(),
            readonly,
            inner: Mutex::new(Inner {
                overlay: BTreeMap::new(),
                changes: Vec::new(),
                finalized: false,
            }),
        })
    }

    fn db_get(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.db.get(key).ok().flatten()
    }

    fn record_change(&self, inner: &mut Inner, typ: &str, key: Vec<u8>, val: Vec<u8>) {
        inner.changes.push(Update {
            typ: typ.to_string(),
            key: String::from_utf8_lossy(&key).into_owned(),
            val,
        });
    }

    /// Iterate every live (post-overlay) key whose bytes start with `prefix`.
    /// Returns owned `(key, value)` pairs in ascending key order.
    fn iter_with_prefix(&self, prefix: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
        let inner = self.inner.lock().unwrap();

        // 1. Pull every matching key from the underlying DB.
        let mut merged: BTreeMap<Vec<u8>, Option<Vec<u8>>> = BTreeMap::new();
        let it = self.db.prefix_iterator(prefix);
        for item in it {
            let Ok((k, v)) = item else { continue };
            if !k.starts_with(prefix) {
                continue;
            }
            merged.insert(k.to_vec(), Some(v.to_vec()));
        }
        // 2. Apply overlay: writes overwrite, tombstones delete.
        for (k, v) in inner.overlay.range(prefix.to_vec()..) {
            if !k.starts_with(prefix) {
                break;
            }
            merged.insert(k.clone(), v.clone());
        }
        // 3. Collect live entries.
        merged
            .into_iter()
            .filter_map(|(k, v)| v.map(|val| (k, val)))
            .collect()
    }

    fn get_value(&self, key: &[u8]) -> Vec<u8> {
        let inner = self.inner.lock().unwrap();
        if let Some(entry) = inner.overlay.get(key) {
            return entry.clone().unwrap_or_default();
        }
        drop(inner);
        self.db_get(key).unwrap_or_default()
    }

    fn has_value(&self, key: &[u8]) -> bool {
        let inner = self.inner.lock().unwrap();
        if let Some(entry) = inner.overlay.get(key) {
            return entry.is_some();
        }
        drop(inner);
        self.db_get(key).is_some()
    }
}

impl Drop for TrxWrapper {
    fn drop(&mut self) {
        // Match Go's "Discard()-on-drop" pattern.
        let mut inner = self.inner.lock().unwrap();
        inner.overlay.clear();
        inner.changes.clear();
        inner.finalized = true;
    }
}

// -- ITrx implementation ---------------------------------------------------

impl ITrx for TrxWrapper {
    fn commit(&self) {
        let mut inner = self.inner.lock().unwrap();
        if inner.finalized || self.readonly {
            inner.finalized = true;
            return;
        }
        // When this instance is part of a geo-distributed cluster, the
        // committed write-set is proposed to the OpenRaft log so every other
        // instance applies the same mutations. `should_replicate` is false
        // for raft-apply threads (no echo) and for scopes the VMM marked as
        // local-only (non-distributed VMs).
        let replicate = crate::drivers::cluster::should_replicate();
        let mut replicated_ops: Vec<crate::drivers::cluster::command::KvOp> = Vec::new();
        let mut batch = WriteBatchWithTransaction::<true>::default();
        for (k, v) in &inner.overlay {
            match v {
                Some(val) => {
                    batch.put(k, val);
                    if replicate {
                        replicated_ops.push(crate::drivers::cluster::command::KvOp::put(
                            String::from_utf8_lossy(k).into_owned(),
                            val,
                        ));
                    }
                }
                None => {
                    batch.delete(k);
                    if replicate {
                        replicated_ops.push(crate::drivers::cluster::command::KvOp::del(
                            String::from_utf8_lossy(k).into_owned(),
                        ));
                    }
                }
            }
        }
        let _ = self.db.write(batch);
        inner.overlay.clear();
        inner.finalized = true;
        drop(inner);
        if !replicated_ops.is_empty() {
            crate::drivers::cluster::on_local_commit(replicated_ops);
        }
    }

    fn discard(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.overlay.clear();
        inner.finalized = true;
    }

    fn get_column(&self, typ: &str, obj_id: &str, column_name: &str) -> Vec<u8> {
        let key = format!("obj::{}::{}::{}", typ, obj_id, column_name);
        self.get_value(key.as_bytes())
    }

    fn del_key(&self, key: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.overlay.insert(key.as_bytes().to_vec(), None);
        let kb = key.as_bytes().to_vec();
        self.record_change(&mut inner, "del", kb, Vec::new());
    }

    fn has_obj(&self, typ: &str, key: &str) -> bool {
        let probe = format!("obj::{}::{}::|", typ, key);
        self.has_value(probe.as_bytes())
    }

    fn get_index(
        &self,
        typ: &str,
        from_column: &str,
        to_column: &str,
        from_column_val: &str,
    ) -> String {
        let key = format!(
            "index::{}::{}::{}::{}",
            typ, from_column, to_column, from_column_val
        );
        String::from_utf8_lossy(&self.get_value(key.as_bytes())).into_owned()
    }

    fn put_index(
        &self,
        typ: &str,
        from_column: &str,
        to_column: &str,
        from_column_val: &str,
        to_column_val: Vec<u8>,
    ) {
        let key = format!(
            "index::{}::{}::{}::{}",
            typ, from_column, to_column, from_column_val
        );
        let mut inner = self.inner.lock().unwrap();
        inner
            .overlay
            .insert(key.as_bytes().to_vec(), Some(to_column_val.clone()));
        self.record_change(&mut inner, "put", key.into_bytes(), to_column_val);
    }

    fn del_index(
        &self,
        typ: &str,
        from_column: &str,
        to_column: &str,
        from_column_val: &str,
    ) {
        let key = format!(
            "index::{}::{}::{}::{}",
            typ, from_column, to_column, from_column_val
        );
        self.del_key(&key);
    }

    fn has_index(
        &self,
        typ: &str,
        from_column: &str,
        to_column: &str,
        from_column_val: &str,
    ) -> bool {
        let key = format!(
            "index::{}::{}::{}::{}",
            typ, from_column, to_column, from_column_val
        );
        self.has_value(key.as_bytes())
    }

    fn get_link(&self, key: &str) -> String {
        let full = format!("link::{}", key);
        String::from_utf8_lossy(&self.get_value(full.as_bytes())).into_owned()
    }

    fn put_link(&self, key: &str, value: &str) {
        let full = format!("link::{}", key);
        let mut inner = self.inner.lock().unwrap();
        inner
            .overlay
            .insert(full.as_bytes().to_vec(), Some(value.as_bytes().to_vec()));
        self.record_change(&mut inner, "put", full.into_bytes(), value.as_bytes().to_vec());
    }

    fn put_bytes(&self, key: &str, value: Vec<u8>) {
        let mut inner = self.inner.lock().unwrap();
        inner
            .overlay
            .insert(key.as_bytes().to_vec(), Some(value.clone()));
        self.record_change(&mut inner, "put", key.as_bytes().to_vec(), value);
    }

    fn get_bytes(&self, key: &str) -> Vec<u8> {
        self.get_value(key.as_bytes())
    }

    fn put_string(&self, key: &str, value: &str) {
        self.put_bytes(key, value.as_bytes().to_vec());
    }

    fn get_string(&self, key: &str) -> String {
        String::from_utf8_lossy(&self.get_value(key.as_bytes())).into_owned()
    }

    fn get_by_prefix(&self, prefix: &str) -> Vec<String> {
        self.iter_with_prefix(prefix.as_bytes())
            .into_iter()
            .map(|(k, _)| String::from_utf8_lossy(&k).into_owned())
            .collect()
    }

    fn get_obj(&self, typ: &str, key: &str) -> HashMap<String, Vec<u8>> {
        let prefix = format!("obj::{}::{}::", typ, key);
        self.iter_with_prefix(prefix.as_bytes())
            .into_iter()
            .map(|(k, v)| {
                let s = String::from_utf8_lossy(&k).into_owned();
                let col = s[prefix.len()..].to_string();
                (col, v)
            })
            .collect()
    }

    fn put_obj(&self, typ: &str, key: &str, mut keys: HashMap<String, Vec<u8>>) {
        keys.insert("|".to_string(), vec![0x01]);
        for (col, val) in keys {
            let full = format!("obj::{}::{}::{}", typ, key, col);
            let mut inner = self.inner.lock().unwrap();
            inner
                .overlay
                .insert(full.as_bytes().to_vec(), Some(val.clone()));
            self.record_change(&mut inner, "put", full.into_bytes(), val);
        }
    }

    fn put_json(&self, key: &str, path: &str, json_obj: &Value, merge: bool) -> Result<()> {
        let m = match json_obj {
            Value::Object(m) => m.clone(),
            other => {
                let s = serde_json::to_string(other)?;
                let parsed: Value = serde_json::from_str(&s)?;
                match parsed {
                    Value::Object(m) => m,
                    _ => return Err(anyhow!("put_json expects an object at the root")),
                }
            }
        };
        self.index_json(key, path, &m, merge)?;
        Ok(())
    }

    fn del_json(&self, key: &str, path: &str) {
        let full = format!("json::{}::{}", key, path);
        self.del_key(&full);
    }

    fn get_json(&self, key: &str, path: &str) -> Result<Map<String, Value>> {
        let full = format!("json::{}::{}", key, path);
        let bytes = self.get_value(full.as_bytes());
        if bytes.is_empty() {
            return Err(anyhow!("json path not found"));
        }
        let m: Map<String, Value> = serde_json::from_slice(&bytes)?;
        Ok(m)
    }

    fn get_links_list(
        &self,
        p: &str,
        _offset: i64,
        _count: i64,
        should_be_global: &[bool],
    ) -> Result<Vec<String>> {
        let prefix = format!("link::{}", p);
        let global = matches!(should_be_global.first(), Some(true));
        let mut out: Vec<String> = Vec::new();
        for (k, _) in self.iter_with_prefix(prefix.as_bytes()) {
            let s = String::from_utf8_lossy(&k).into_owned();
            if global && !s.ends_with("@global") {
                continue;
            }
            out.push(s["link::".len()..].to_string());
        }
        Ok(out)
    }

    fn search_link_vals_list(
        &self,
        typ: &str,
        from_column: &str,
        to_column: &str,
        word: &str,
        filter: &HashMap<String, String>,
        offset: i64,
        count: i64,
    ) -> Result<Vec<String>> {
        let prefix = format!("index::{}::{}::{}::", typ, from_column, to_column);
        let mut out: Vec<String> = Vec::new();
        let mut counter: i64 = 0;
        for (k, v) in self.iter_with_prefix(prefix.as_bytes()) {
            let key_str = String::from_utf8_lossy(&k[prefix.len()..]).into_owned();
            if !key_str.contains(word) {
                continue;
            }
            let val_str = String::from_utf8_lossy(&v).into_owned();
            let mut matched = true;
            for (fk, fv) in filter {
                if String::from_utf8_lossy(&self.get_column(typ, &val_str, fk)) != *fv {
                    matched = false;
                    break;
                }
            }
            if !matched {
                continue;
            }
            if counter < offset {
                counter += 1;
                continue;
            }
            if counter >= offset + count {
                break;
            }
            out.push(val_str);
            counter += 1;
        }
        Ok(out)
    }

    fn search_link_keys_list_by_prefix(
        &self,
        p: &str,
        typ: &str,
        filter: &HashMap<String, String>,
        in_arr_filter: &HashMap<String, Vec<String>>,
        offset: i64,
        count: i64,
        should_be_global: &[bool],
    ) -> Result<Vec<String>> {
        let prefix = format!("link::{}", p);
        let global = matches!(should_be_global.first(), Some(true));
        let mut out: Vec<String> = Vec::new();
        let mut counter: i64 = 0;
        for (k, _) in self.iter_with_prefix(prefix.as_bytes()) {
            let s = String::from_utf8_lossy(&k).into_owned();
            if global && !s.ends_with("@global") {
                continue;
            }
            let obj_id = s[prefix.len()..].to_string();
            let mut matched = true;
            for (fk, fv) in filter {
                if String::from_utf8_lossy(&self.get_column(typ, &obj_id, fk)) != *fv {
                    matched = false;
                    break;
                }
            }
            if !matched {
                continue;
            }
            for (fk, vals) in in_arr_filter {
                let probe = String::from_utf8_lossy(&self.get_column(typ, &obj_id, fk)).into_owned();
                if !vals.iter().any(|v| v == &probe) {
                    matched = false;
                    break;
                }
            }
            if !matched {
                continue;
            }
            if counter < offset {
                counter += 1;
                continue;
            }
            if counter >= offset + count {
                break;
            }
            out.push(obj_id);
            counter += 1;
        }
        Ok(out)
    }

    fn get_obj_list(
        &self,
        typ: &str,
        obj_ids: &[String],
        query: &HashMap<String, String>,
        meta: &[i64],
    ) -> Result<HashMap<String, HashMap<String, Vec<u8>>>> {
        // Fast path: explicit id list.
        if !(obj_ids.len() == 1 && obj_ids[0] == "*") {
            let mut out = HashMap::new();
            for id in obj_ids {
                let entry = self.get_obj(typ, id);
                out.insert(id.clone(), entry);
            }
            return Ok(out);
        }

        // Iterator path: gather every object whose column-map matches `query`,
        // honouring `meta == [offset]` or `meta == [offset, count]`.
        let prefix = format!("obj::{}::", typ);
        let entries = self.iter_with_prefix(prefix.as_bytes());

        let mut grouped: BTreeMap<String, HashMap<String, Vec<u8>>> = BTreeMap::new();
        for (k, v) in entries {
            let s = String::from_utf8_lossy(&k).into_owned();
            let tail = &s[prefix.len()..];
            let mut parts = tail.splitn(2, "::");
            let id = match parts.next() {
                Some(s) => s.to_string(),
                None => continue,
            };
            let col = parts.next().unwrap_or("").to_string();
            grouped.entry(id).or_default().insert(col, v);
        }

        let mut out: HashMap<String, HashMap<String, Vec<u8>>> = HashMap::new();
        let offset = meta.first().copied().unwrap_or(0);
        let count = meta.get(1).copied();
        let mut index: i64 = 0;
        for (id, cols) in grouped {
            if !cols.contains_key("|") {
                continue;
            }
            let mut matched = true;
            for (k, v) in query {
                let probe = cols.get(k).cloned().unwrap_or_default();
                if String::from_utf8_lossy(&probe) != *v {
                    matched = false;
                    break;
                }
            }
            if !matched {
                continue;
            }
            if index < offset {
                index += 1;
                continue;
            }
            if let Some(c) = count {
                if index >= offset + c {
                    break;
                }
            }
            index += 1;
            out.insert(id, cols);
        }
        Ok(out)
    }

    fn get_pri_key(&self, tag: &str) -> Option<RsaPrivateKey> {
        let res = self.get_string(&format!("obj::User::{}::privateKey", tag));
        if res.is_empty() {
            return None;
        }
        cryp::parse_private_key(res.as_bytes()).ok()
    }

    fn get_pub_key(&self, tag: &str) -> Option<RsaPublicKey> {
        // Creature is the single authoritative identity record; fall back to the
        // legacy User mirror for data written before the unification.
        let mut res = self.get_string(&format!("obj::Creature::{}::publicKey", tag));
        if res.is_empty() {
            res = self.get_string(&format!("obj::User::{}::publicKey", tag));
        }
        if res.is_empty() {
            return None;
        }
        cryp::parse_public_key(res.as_bytes()).ok()
    }

    fn updates(&self) -> Vec<Update> {
        self.inner.lock().unwrap().changes.clone()
    }
}

impl TrxWrapper {
    /// Mirrors `tw.indexJson` — recursively splat a JSON object so each
    /// leaf path can be addressed independently. Honours `merge` by reading
    /// the existing path and folding the new value on top.
    fn index_json(
        &self,
        key: &str,
        path: &str,
        obj: &Map<String, Value>,
        merge: bool,
    ) -> Result<()> {
        let mut old: Map<String, Value> = Map::new();
        if merge {
            if let Ok(existing) = self.get_json(key, path) {
                old = existing;
            }
        }
        let mut sorted_keys: Vec<&String> = obj.keys().collect();
        sorted_keys.sort();

        merge_objects(&mut old, obj);
        let bytes = serde_json::to_vec(&old)?;
        self.put_bytes(&format!("json::{}::{}", key, path), bytes);

        for k in sorted_keys {
            let v = &obj[k];
            if v.is_null() {
                continue;
            }
            match v {
                Value::Object(child) => {
                    self.index_json(key, &format!("{}.{}", path, k), child, merge)?;
                }
                other => {
                    let bytes = serde_json::to_vec(other)?;
                    self.put_bytes(&format!("json::{}::{}.{}", key, path, k), bytes);
                }
            }
        }
        Ok(())
    }

    /// Iterator-only access to the underlying DB for callers that need raw
    /// key/value pairs without overlay filtering (e.g. recovery / debug).
    pub fn raw_db_iterator(&self) -> Vec<(Vec<u8>, Vec<u8>)> {
        let mut out = Vec::new();
        for item in self.db.iterator(IteratorMode::Start) {
            if let Ok((k, v)) = item {
                out.push((k.to_vec(), v.to_vec()));
            }
        }
        out
    }
}

fn merge_objects(dst: &mut Map<String, Value>, src: &Map<String, Value>) {
    for (k, v) in src {
        match v {
            Value::Object(m_src) => {
                let need_recurse = matches!(dst.get(k), Some(Value::Object(_)));
                if need_recurse {
                    if let Some(Value::Object(m_dst)) = dst.get_mut(k) {
                        merge_objects(m_dst, m_src);
                    }
                } else {
                    dst.insert(k.clone(), Value::Object(m_src.clone()));
                }
            }
            _ => {
                dst.insert(k.clone(), v.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ports::file::IFile;
    use crate::models::ports::network::INetwork;
    use crate::models::ports::security::ISecurity;
    use crate::models::ports::signaler::ISignaler;
    use crate::models::ports::tools::ITools;
    use crate::models::ports::vmm::IVmm;
    use std::sync::Arc;

    // ---- minimal `ICore` stub for unit tests -------------------------------

    struct StubCore {
        storage: Arc<dyn IStorage>,
    }

    impl ICore for StubCore {
        fn owner_id(&self) -> String { String::new() }
        fn id(&self) -> String { "test-node".into() }
        fn gods(&self) -> Vec<String> { Vec::new() }
        fn add_god(&self, _: &str) {}
        fn tools(&self) -> Arc<dyn ITools> {
            // Tests never call core.tools() — provide a panic-on-use stub.
            unimplemented!("tools() not used by trx tests");
        }
        fn free_nodes(&self) -> HashMap<String, bool> { HashMap::new() }
        fn add_free_node(&self, _: &str) {}
        fn actor(&self) -> Arc<dyn crate::models::action::actor::IActor> {
            unimplemented!()
        }
        fn load(&self, _: Vec<String>, _: HashMap<String, Value>) {}
        fn close(&self) {}
        fn plant_chain_trigger(&self, _: i64, _: &str, _: &str, _: &str, _: &str, _: &str) {}
        fn app_pending_trxs(&self) {}
        fn ip_addr(&self) -> String { String::new() }
        fn modify_state(
            &self,
            _: bool,
            mut fn_: crate::models::action::TrxClosure,
        ) {
            let tw = TrxWrapper::new(
                Arc::new(StubCore { storage: self.storage.clone() }),
                self.storage.clone(),
                true,
            );
            let _ = fn_(&*tw);
        }
        fn modify_state_securly_with_source(
            &self,
            _: bool,
            _: Arc<dyn crate::models::info::IInfo>,
            _: &str,
            _: crate::models::core::StateClosure,
        ) {
        }
        fn modify_state_securly(
            &self,
            _: bool,
            _: Arc<dyn crate::models::info::IInfo>,
            _: crate::models::core::StateClosure,
        ) {
        }
        fn sign_packet(&self, _: &[u8]) -> String { String::new() }
        fn sign_packet_as_owner(&self, _: &[u8]) -> String { String::new() }
        fn execution_cost_per_second(&self) -> i64 { 0 }
        fn vm_ram_cost_per_mb_per_minute(&self) -> i64 { 0 }
        fn vm_cpu_core_cost_per_minute(&self) -> i64 { 0 }
        fn vm_disk_cost_per_gb_per_minute(&self) -> i64 { 0 }
        fn globe(&self) -> Arc<dyn crate::models::globe::IGlobe> {
            unimplemented!()
        }
        fn begin_vm_trx(&self, _vm_id: &str) -> Arc<dyn ITrx> {
            unimplemented!("begin_vm_trx not used in trx unit tests");
        }
        fn end_vm_trx(&self, _vm_id: &str) {}
    }

    struct StubStorage {
        root: String,
        kv: crate::models::ports::storage::KvDb,
        ts: crate::models::ports::storage::TsDb,
    }

    impl StubStorage {
        fn new() -> Arc<Self> {
            let dir = format!(
                "/tmp/caspar-trx-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            );
            std::fs::create_dir_all(&dir).unwrap();
            let kv: crate::models::ports::storage::KvDb = Arc::new(
                rocksdb::TransactionDB::open_default(&dir).expect("rocksdb"),
            );
            // The tests never touch ts_db, but the trait requires a value.
            // Build an unconnected pool — get() will fail but no test calls it.
            let manager = r2d2_postgres::PostgresConnectionManager::new(
                "host=127.0.0.1 port=1 user=x dbname=x".parse().unwrap(),
                postgres::tls::NoTls,
            );
            let ts = r2d2::Builder::new().build_unchecked(manager);
            Arc::new(StubStorage { root: dir, kv, ts })
        }
    }

    impl IStorage for StubStorage {
        fn storage_root(&self) -> String { self.root.clone() }
        fn kv_db(&self) -> crate::models::ports::storage::KvDb { self.kv.clone() }
        fn ts_db(&self) -> crate::models::ports::storage::TsDb { self.ts.clone() }
        fn gen_id(&self, _t: &dyn ITrx, _: &str) -> String { String::new() }
        fn log_time_sieries(
            &self, _: &str, _: &str, _: &str, _: i64,
        ) -> crate::models::packet::LogPacket { Default::default() }
        fn update_log(
            &self, _: &str, _: &str, _: &str, _: &str, _: i64,
        ) -> crate::models::packet::LogPacket { Default::default() }
        fn read_store_logs(&self, _: &str, _: i64, _: i64) -> Vec<crate::models::packet::LogPacket> { Vec::new() }
        fn pick_store_logs(&self, _: &str, _: Vec<String>) -> Vec<crate::models::packet::LogPacket> { Vec::new() }
        fn log_vm(&self, _: &str, _: &str, _: &str, _: i64) -> crate::models::packet::BuildPacket { Default::default() }
        fn read_vm_logs(&self, _: &str, _: &str, _: i64, _: i64) -> Vec<crate::models::packet::BuildPacket> { Vec::new() }
    }

    fn fresh_trx(readonly: bool) -> (Arc<dyn IStorage>, Arc<TrxWrapper>) {
        let storage: Arc<dyn IStorage> = StubStorage::new();
        let core: Arc<dyn ICore> = Arc::new(StubCore { storage: storage.clone() });
        let tw = TrxWrapper::new(core, storage.clone(), readonly);
        (storage, tw)
    }

    #[test]
    fn put_get_overlay() {
        let (_s, tw) = fresh_trx(false);
        tw.put_string("k1", "v1");
        assert_eq!(tw.get_string("k1"), "v1");
        tw.del_key("k1");
        assert_eq!(tw.get_string("k1"), "");
    }

    #[test]
    fn commit_persists() {
        let (storage, tw) = fresh_trx(false);
        tw.put_string("persist", "yes");
        tw.commit();
        let tw2 = TrxWrapper::new(
            Arc::new(StubCore { storage: storage.clone() }),
            storage,
            true,
        );
        assert_eq!(tw2.get_string("persist"), "yes");
    }

    #[test]
    fn obj_put_get_roundtrip() {
        let (_s, tw) = fresh_trx(false);
        let mut cols = HashMap::new();
        cols.insert("a".to_string(), b"1".to_vec());
        cols.insert("b".to_string(), b"2".to_vec());
        tw.put_obj("User", "u1", cols);
        let read = tw.get_obj("User", "u1");
        assert_eq!(read.get("a").map(|v| v.as_slice()), Some(&b"1"[..]));
        assert_eq!(read.get("b").map(|v| v.as_slice()), Some(&b"2"[..]));
        assert!(read.contains_key("|"));
        assert!(tw.has_obj("User", "u1"));
    }

    #[test]
    fn json_indexing_splats_paths() {
        let (_s, tw) = fresh_trx(false);
        let v: Value = serde_json::json!({"a": 1, "b": {"c": "x"}});
        tw.put_json("k", "p", &v, false).unwrap();
        let leaf = tw.get_bytes("json::k::p.b.c");
        assert_eq!(leaf, br#""x""#);
        let parent = tw.get_json("k", "p.b").unwrap();
        assert_eq!(parent.get("c"), Some(&Value::String("x".into())));
    }

    #[test]
    fn discard_drops_changes() {
        let (storage, tw) = fresh_trx(false);
        tw.put_string("temp", "v");
        tw.discard();
        // A fresh wrapper must not see the put.
        let tw2 = TrxWrapper::new(
            Arc::new(StubCore { storage: storage.clone() }),
            storage,
            true,
        );
        assert_eq!(tw2.get_string("temp"), "");
    }

    #[test]
    fn read_after_delete_returns_empty_within_same_trx() {
        let (_s, tw) = fresh_trx(false);
        tw.put_string("k", "v");
        assert_eq!(tw.get_string("k"), "v");
        tw.del_key("k");
        // Tombstone is visible to subsequent reads in the same transaction.
        assert_eq!(tw.get_string("k"), "");
        // Bytes path also sees the tombstone.
        assert!(tw.get_bytes("k").is_empty());
    }

    #[test]
    fn updates_capture_overlay_writes_and_deletes() {
        let (_s, tw) = fresh_trx(false);
        tw.put_string("k1", "v1");
        tw.put_bytes("k2", b"raw".to_vec());
        tw.del_key("k1");
        let updates = tw.updates();
        // Three changes captured in order.
        assert_eq!(updates.len(), 3);
    }

    #[test]
    fn put_link_round_trips_through_overlay() {
        let (_s, tw) = fresh_trx(false);
        tw.put_link("alpha", "beta");
        assert_eq!(tw.get_link("alpha"), "beta");
    }

    #[test]
    fn link_value_indexing_round_trips() {
        let (_s, tw) = fresh_trx(false);
        tw.put_index("User", "id", "email", "1", b"a@b".to_vec());
        assert!(tw.has_index("User", "id", "email", "1"));
        assert_eq!(tw.get_index("User", "id", "email", "1"), "a@b");
        tw.del_index("User", "id", "email", "1");
        assert!(!tw.has_index("User", "id", "email", "1"));
    }

    #[test]
    fn second_commit_is_noop() {
        let (storage, tw) = fresh_trx(false);
        tw.put_string("x", "1");
        tw.commit();
        // A second commit on the same wrapper must not double-apply nor panic.
        tw.commit();
        let tw2 = TrxWrapper::new(
            Arc::new(StubCore { storage: storage.clone() }),
            storage,
            true,
        );
        assert_eq!(tw2.get_string("x"), "1");
    }

    #[test]
    fn json_merge_preserves_unrelated_paths() {
        let (_s, tw) = fresh_trx(false);
        let initial = serde_json::json!({"a": 1, "b": {"c": "x", "d": "y"}});
        tw.put_json("k", "p", &initial, false).unwrap();
        // Merge a new leaf inside `b`; existing `c` should remain.
        let patch = serde_json::json!({"b": {"e": "z"}});
        tw.put_json("k", "p", &patch, true).unwrap();
        let merged = tw.get_json("k", "p").unwrap();
        let b = merged.get("b").unwrap().as_object().unwrap();
        assert_eq!(b.get("c"), Some(&Value::String("x".into())));
        assert_eq!(b.get("d"), Some(&Value::String("y".into())));
        assert_eq!(b.get("e"), Some(&Value::String("z".into())));
    }

    // Suppress unused-import warnings on driver traits referenced by the
    // stub `ICore` impl path.
    #[allow(dead_code)]
    fn _silence(_: Option<Arc<dyn IFile>>, _: Option<Arc<dyn INetwork>>, _: Option<Arc<dyn ISecurity>>, _: Option<Arc<dyn ISignaler>>, _: Option<Arc<dyn IVmm>>) {}
}
