//! Translation of the Go `kasper/src/abstract` package tree.
//!
//! The Go package was named `abstract`, which is a reserved word in Rust, so
//! the module is renamed to `abstractions`. Every Go file maps to a Rust file
//! of the same name; package-level `mod.rs` files re-export their members so
//! that, e.g., `abstractions::adapters::network::IChain` resolves exactly like
//! the Go `network.IChain` did.

pub mod adapters;
pub mod models;
pub mod state;
