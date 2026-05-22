//! Translation of `kasper/src/core/module`.
//!
//! Phase 1 translated the `logger` module; Phase 3 adds `pool`, `actor` and
//! its model subtree (`base`, `secured`, `state`). The `globe` and the full
//! `core` orchestrator follow in subsequent commits as their driver
//! dependencies land.

pub mod actor;
pub mod globe;
pub mod logger;
pub mod pool;
