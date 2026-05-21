//! Translation of `abstract/models/input/input.go`.

/// An action input payload. Concrete inputs live under `shell/api/inputs`.
pub trait IInput: Send + Sync {
    fn get_store_id(&self) -> String;
    fn origin(&self) -> String;
}
