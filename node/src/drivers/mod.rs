//! Driver adapters — concrete implementations of the port traits defined
//! in [`crate::abstractions::ports`].
//!
//! - `file` — filesystem-backed [`crate::abstractions::ports::file::IFile`]
//! - `signaler` — in-process pub/sub event bus
//! - `storage` — RocksDB + PostgreSQL persistence
//! - `security` — RSA/ECDSA signing and verification
//! - `vmm` — in-process virtual-machine driver (wasm / docker /
//!   javascript / elpify / elpian / firecracker)
//! - `network` — chain consensus + TCP/WS client + federation transports

pub mod file;
pub mod network;
pub mod security;
pub mod signaler;
pub mod storage;
pub mod vmm;
