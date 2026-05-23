//! Translation of `shell/api/actions/dummy/dummy.go`.

use std::env;
use std::sync::Arc;

use anyhow::Result;
use serde_json::{json, Value};

use crate::abstractions::models::action::action::ISecureAction;
use crate::abstractions::models::core::ICore;
use crate::abstractions::state::IState;
use crate::core::module::actor::model::secured::guard::Guard;
use crate::shell::api::inputs::simple::HelloInput;
use crate::shell::utils::future::async_once;

use super::util::build_secure_action;

pub fn hello(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<HelloInput, _>(
        app,
        "/api/hello",
        Guard::default(),
        move |_state: Arc<dyn IState>, input: HelloInput| -> Result<Value> {
            Ok(json!({"message": format!("hello {} !", input.name)}))
        },
    )
}

pub fn time(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<HelloInput, _>(
        app,
        "/api/time",
        Guard::default(),
        move |_state: Arc<dyn IState>, _: HelloInput| -> Result<Value> {
            let now = chrono::Utc::now().timestamp_millis();
            Ok(json!({"time": now}))
        },
    )
}

pub fn ping(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<HelloInput, _>(
        app,
        "/api/ping",
        Guard::default(),
        move |_state: Arc<dyn IState>, _: HelloInput| -> Result<Value> {
            // Detach a background no-op task so the future helper is wired
            // and exercised (mirrors how the Go shell pinged future.Async).
            let _h = async_once(|| {});
            Ok(json!(env::var("MAIN_PORT").unwrap_or_default()))
        },
    )
}

pub fn install(app: Arc<dyn ICore>) {
    let actor = app.actor();
    for a in [hello(app.clone()), time(app.clone()), ping(app.clone())] {
        actor.inject_secure_action(a);
    }
}
