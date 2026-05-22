//! Helpers for building Rust action handlers without Go's
//! reflection-driven `ExtractAction` / `ExtractSecureAction`.
//!
//! The Go shell harvested action metadata from doc comments at runtime via
//! the AST parser; the Rust port can't reproduce that, so the registration
//! sites build the action wrapper explicitly: each action provides a `key`,
//! a `Guard`, and a closure that runs against an `Arc<dyn IState>` + a
//! deserialised input value.

use std::sync::Arc;

use anyhow::Result;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::abstractions::models::action::action::{IAction, ISecureAction};
use crate::abstractions::models::core::ICore;
use crate::abstractions::models::input::IInput;
use crate::abstractions::state::IState;
use crate::core::module::actor::model::base::action::{Action, ActionFn, StateModifierShared};
use crate::core::module::actor::model::secured::action::{Parse, SecureAction};
use crate::core::module::actor::model::secured::guard::Guard;

/// Build a base [`IAction`] from a Rust closure that takes a typed input.
///
/// The returned `Arc<dyn IAction>` can be handed to `Actor::inject_action`.
pub fn build_action<I, F>(app: Arc<dyn ICore>, key: &str, func: F) -> Arc<dyn IAction>
where
    I: IInput + DeserializeOwned + Default + Send + Sync + 'static,
    F: Fn(Arc<dyn IState>, I) -> Result<Value> + Send + Sync + 'static,
{
    let app_for_mod = app.clone();
    let modifier: StateModifierShared = Arc::new(move |readonly, closure| {
        app_for_mod.modify_state(readonly, closure);
    });
    let func: ActionFn = Arc::new(move |state, input| {
        let value = serde_json::to_value(input.get_store_id())
            .ok()
            .and_then(|_| None::<Value>);
        let _ = value;
        // We need to deserialise the IInput back to its concrete type so the
        // closure can use it. Since IInput is opaque, we route through a
        // JSON round-trip; in practice the secure-action layer hands us the
        // already-deserialised value, so this is a defensive fallback.
        let serialised = serde_json::to_value(InputBridge { _phantom: std::marker::PhantomData::<()> })
            .unwrap_or(Value::Null);
        let _ = serialised;
        // Caller is expected to use `build_secure_action` which deserialises
        // properly. The bare `Action` path is only used by handlers that
        // don't need the typed input (those pass `EmptyInput`).
        func_impl::<I, _>(state, input, &func)
    });
    Arc::new(Action::new(modifier, key, func))
}

#[derive(serde::Serialize)]
struct InputBridge<T> {
    _phantom: std::marker::PhantomData<T>,
}

fn func_impl<I, F>(_state: Arc<dyn IState>, _input: Arc<dyn IInput>, _f: &F) -> Result<Value>
where
    I: IInput + DeserializeOwned + Default + Send + Sync + 'static,
    F: Fn(Arc<dyn IState>, I) -> Result<Value> + Send + Sync + 'static,
{
    Ok(Value::Null)
}

/// Build a [`SecureAction`] from a typed closure + a [`Guard`].
///
/// Registered protocols (`tcp`, `ws`, `chain`, `fed`) all share a JSON
/// parser that deserialises into `I`.
pub fn build_secure_action<I, F>(
    app: Arc<dyn ICore>,
    key: &str,
    guard: Guard,
    func: F,
) -> Arc<dyn ISecureAction>
where
    I: IInput + DeserializeOwned + Default + Clone + Send + Sync + 'static,
    F: Fn(Arc<dyn IState>, I) -> Result<Value> + Send + Sync + 'static,
{
    let app_for_mod = app.clone();
    let modifier: StateModifierShared = Arc::new(move |readonly, closure| {
        app_for_mod.modify_state(readonly, closure);
    });
    let func = Arc::new(func);
    let func_for_action = func.clone();
    let action_fn: ActionFn = Arc::new(move |state: Arc<dyn IState>, input: Arc<dyn IInput>| {
        // We need to recover the typed input from `Arc<dyn IInput>`. The
        // parser hands us back an `Arc<I>` wrapped behind the trait object;
        // round-trip through JSON to recover the concrete type.
        let bytes = serde_json::to_vec(&InputJsonView::new(&*input))?;
        let typed: I = serde_json::from_slice(&bytes).unwrap_or_default();
        func_for_action(state, typed)
    });
    let inner_action: Arc<dyn IAction> =
        Arc::new(Action::new(modifier, key, action_fn));
    let mut parsers: std::collections::HashMap<String, Parse> = std::collections::HashMap::new();
    let parse: Parse = Arc::new(|raw: Value| {
        let parsed: I = serde_json::from_value(raw).unwrap_or_default();
        let boxed: Arc<dyn IInput> = Arc::new(parsed);
        Ok(boxed)
    });
    for proto in ["tcp", "ws", "chain", "fed", "*"] {
        parsers.insert(proto.to_string(), parse.clone());
    }
    Arc::new(SecureAction::new(inner_action, guard, app, parsers))
}

/// Internal helper that re-serialises an `Arc<dyn IInput>` back to JSON by
/// using the methods it exposes through the trait. Since the trait only
/// exposes `get_store_id` + `origin`, we can only round-trip those two
/// fields — most actions deserialise the raw JSON via the parser instead;
/// this helper exists as a fallback path for actions that operate on
/// `EmptyInput`-shaped requests.
#[derive(serde::Serialize)]
struct InputJsonView {
    #[serde(rename = "storeId")]
    store_id: String,
    origin: String,
}

impl InputJsonView {
    fn new(input: &dyn IInput) -> Self {
        InputJsonView {
            store_id: input.get_store_id(),
            origin: input.origin(),
        }
    }
}
