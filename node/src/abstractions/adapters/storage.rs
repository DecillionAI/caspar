//! Translation of `abstract/adapters/storage/storage.go`.
//!
//! The Go node used Badger for the key/value database and `database/sql`
//! (pgx → QuestDB) for the time-series database. The Rust translation uses a
//! RocksDB transactional database in place of Badger, and an `r2d2`-pooled
//! synchronous PostgreSQL-wire client for QuestDB.

use std::sync::Arc;

use crate::abstractions::models::packet::{BuildPacket, LogPacket};
use crate::abstractions::models::trx::ITrx;

/// Key/value database handle — RocksDB transactional DB (replaces Badger).
pub type KvDb = Arc<rocksdb::TransactionDB>;

/// Time-series database handle — pooled QuestDB (PostgreSQL-wire) client.
pub type TsDb = r2d2::Pool<r2d2_postgres::PostgresConnectionManager<postgres::tls::NoTls>>;

/// The storage driver interface.
pub trait IStorage: Send + Sync {
    fn storage_root(&self) -> String;
    fn kv_db(&self) -> KvDb;
    fn ts_db(&self) -> TsDb;
    fn gen_id(&self, t: &dyn ITrx, origin: &str) -> String;
    fn log_time_sieries(
        &self,
        store_id: &str,
        user_id: &str,
        data: &str,
        time_val: i64,
    ) -> LogPacket;
    fn update_log(
        &self,
        store_id: &str,
        user_id: &str,
        signal_id: &str,
        data: &str,
        time_val: i64,
    ) -> LogPacket;
    fn read_store_logs(&self, store_id: &str, before_time: i64, count: i64) -> Vec<LogPacket>;
    fn pick_store_logs(&self, store_id: &str, ids: Vec<String>) -> Vec<LogPacket>;
    fn log_vm(
        &self,
        vm_id: &str,
        log_type: &str,
        data: &str,
        time_val: i64,
    ) -> BuildPacket;
    fn read_vm_logs(
        &self,
        vm_id: &str,
        log_type: &str,
        offset: i64,
        count: i64,
    ) -> Vec<BuildPacket>;
}
