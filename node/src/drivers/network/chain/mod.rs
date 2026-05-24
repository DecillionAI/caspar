//! Babble hashgraph consensus stack and the Caspar `IChain` driver built on
//! top of it.
//!
//! Layering (bottom-up):
//!   * `common`, `crypto`, `peers` — primitives and validator set.
//!   * `hashgraph` — Leemon Baird's hashgraph algorithm + event store.
//!   * `net` — TCP / in-memory transports for the gossip layer.
//!   * `node` — the consensus node (gossip loop, RPC handlers, state).
//!   * `proxy`, `dummy` — app-side bridges used in production and tests.
//!   * `config`, `babble` — assembly of the above into a Babble engine.
//!   * `blockchain` — the `IChain` driver wrapping the Babble engine.

pub mod babble;
pub mod blockchain;
pub mod common;
pub mod config;
pub mod crypto;
pub mod dummy;
pub mod hashgraph;
pub mod net;
pub mod node;
pub mod peers;
pub mod proxy;

pub use blockchain::{Blockchain, CliConfig};
