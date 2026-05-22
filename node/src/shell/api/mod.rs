//! Translation of `shell/api` — request/response/update payload models and
//! the action handlers that turn them into state mutations.
//!
//! Phase 4 lands the persisted-entity `model` tree (used by drivers + actions)
//! ahead of the rest of the API. The `inputs`, `outputs`, `actions`,
//! `pluggers`, `updates`, and `main` submodules follow once each cluster is
//! translated; `extractor.go` / `doc.go` are intentionally skipped — they
//! relied on Go's AST/runtime reflection and have no Rust analogue (a derive
//! macro will take their place when the action registry lands).

pub mod inputs;
pub mod model;
pub mod outputs;
pub mod updates;
