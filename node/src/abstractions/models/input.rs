//! Translation of `abstract/models/input/input.go`.
//!
//! Adds `as_any` on top of the Go interface so the secured-action layer can
//! downcast `Arc<dyn IInput>` back to its concrete type without a JSON
//! round-trip (which would lose every untagged field).

use std::any::Any;

/// An action input payload. Concrete inputs live under `shell/api/inputs`.
pub trait IInput: Send + Sync + Any {
    fn get_store_id(&self) -> String;
    fn origin(&self) -> String;

    /// Downcast hook — every concrete implementation should return
    /// `self` so `Arc<dyn IInput>` can recover the original typed value.
    fn as_any(&self) -> &dyn Any;
}
