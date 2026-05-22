//! Translation of `core/module/actor` — the action dispatch system.
//!
//! The submodules mirror the Go layout one-for-one:
//!   * `actor.rs`            — `actor.go`: the [`IActor`] registry.
//!   * `model/base/info.rs`  — `model/base/info.go`: the identity context.
//!   * `model/base/action.rs`— `model/base/action.go`: the simple [`IAction`].
//!   * `model/state/state.rs`— `model/state/state.go`: the [`IState`] carrier.
//!   * `model/secured/guard.rs` + `model/secured/action.rs` — the secured
//!     action layer that authenticates callers and routes requests across
//!     federation / chain when the origin is remote.
//!
//! `model/trx/trx.rs` (the RocksDB-backed [`ITrx`] implementation) waits on
//! the storage driver from Phase 4.

pub mod actor;
pub mod model;

pub use actor::Actor;
pub use model::base::action::Action;
pub use model::base::info::Info;
pub use model::secured::action::SecureAction;
pub use model::secured::guard::Guard;
pub use model::state::state::State;
