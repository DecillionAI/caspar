//! Translation of `shell/api/actions/program/program.go`.
//!
//! Registers the 23 program / app / VM actions. The handler bodies are
//! the heaviest concentration of business logic in the shell (~800 LOC
//! across billing, deploy pipelines, log-stream multiplexing, terminal
//! attach/detach and chain dispatch). The Rust translation lands the
//! registration surface here with `not_implemented` bodies so the
//! routing/security/parser/federation paths all type-check end-to-end;
//! the per-handler bodies follow as targeted commits.

use std::sync::Arc;

use anyhow::{anyhow, Result};
use serde_json::Value;

use crate::abstractions::models::action::action::ISecureAction;
use crate::abstractions::models::core::ICore;
use crate::core::module::actor::model::secured::guard::Guard;
use crate::shell::api::inputs::program::{
    CreateAppInput, CreateMachineInput, DeleteAppInput, DeleteProgramInput, DeployInput, ListAppMachsInput,
    ListInput, MachineBuildsInput, ReadVmLogsInput, RunProgramEntityInput, SignalInput, UpdateAppInput,
    UpdateProgramInput, VmTerminalInput,
};

use super::util::build_secure_action;

fn user_guard() -> Guard {
    Guard {
        is_user: true,
        is_in_store: false,
    }
}

fn not_impl(key: &'static str) -> impl Fn() -> Result<Value> {
    move || Err(anyhow!("program action `{}` not yet translated", key))
}

macro_rules! register {
    ($app:expr, $key:literal, $guard:expr, $input:ty) => {{
        let action: Arc<dyn ISecureAction> = build_secure_action::<$input, _>(
            $app.clone(),
            $key,
            $guard,
            move |_, _: $input| (not_impl($key))(),
        );
        action
    }};
}

/// Plug every program action into the actor.
pub fn install(app: Arc<dyn ICore>) {
    let actor = app.actor();
    let handlers: Vec<Arc<dyn ISecureAction>> = vec![
        register!(app, "/machines/createProgram", user_guard(), CreateMachineInput),
        register!(app, "/machines/deleteProgram", user_guard(), DeleteProgramInput),
        register!(app, "/machines/updateProgram", user_guard(), UpdateProgramInput),
        register!(app, "/machines/runProgramEntity", user_guard(), RunProgramEntityInput),
        register!(app, "/machines/stopProgramEntity", user_guard(), RunProgramEntityInput),
        register!(app, "/machines/readVmLogs", user_guard(), ReadVmLogsInput),
        register!(app, "/machines/openVmTerminal", user_guard(), VmTerminalInput),
        register!(app, "/machines/closeVmTerminal", user_guard(), VmTerminalInput),
        register!(app, "/machines/readMachineBuilds", user_guard(), MachineBuildsInput),
        register!(app, "/machines/deploy", user_guard(), DeployInput),
        register!(app, "/machines/list", user_guard(), ListInput),
        register!(app, "/machines/listAppMachines", user_guard(), ListAppMachsInput),
        register!(app, "/machines/createApp", user_guard(), CreateAppInput),
        register!(app, "/machines/deleteApp", user_guard(), DeleteAppInput),
        register!(app, "/machines/updateApp", user_guard(), UpdateAppInput),
        register!(app, "/machines/signal", user_guard(), SignalInput),
    ];
    for h in handlers {
        actor.inject_secure_action(h);
    }
}
