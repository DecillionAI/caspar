//! Translation of `shell/api/actions/auth/auth.go`.

use std::sync::Arc;

use anyhow::Result;
use serde_json::{json, Value};

use crate::abstractions::models::action::ISecureAction;
use crate::abstractions::models::core::ICore;
use crate::abstractions::state::IState;
use crate::core::actor::model::secured::guard::Guard;
use crate::shell::api::inputs::auth::{GetServerKeyInput, GetServersMapInput};
use crate::shell::api::outputs::auth::{GetServerKeyOutput, GetServersMapOutput};

use super::util::build_secure_action;

pub fn get_server_public_key(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    let app_for_handler = app.clone();
    build_secure_action::<GetServerKeyInput, _>(
        app,
        "/auths/getServerPublicKey",
        Guard::default(),
        move |_state: Arc<dyn IState>, _: GetServerKeyInput| -> Result<Value> {
            let pair = app_for_handler.tools().security().fetch_key_pair("server_key");
            let public_key = pair
                .get(1)
                .map(|p| String::from_utf8_lossy(p).into_owned())
                .unwrap_or_default();
            Ok(json!(GetServerKeyOutput { public_key }))
        },
    )
}

pub fn get_servers_map(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    let app_for_handler = app.clone();
    build_secure_action::<GetServersMapInput, _>(
        app,
        "/auths/getServersMap",
        Guard::default(),
        move |_state: Arc<dyn IState>, _: GetServersMapInput| -> Result<Value> {
            let servers = app_for_handler.tools().network().chain().peers();
            Ok(json!(GetServersMapOutput { servers }))
        },
    )
}

/// Plug every auth action into the actor.
pub fn install(app: Arc<dyn ICore>) {
    let actor = app.actor();
    let actions: Vec<Arc<dyn ISecureAction>> = vec![
        get_server_public_key(app.clone()),
        get_servers_map(app.clone()),
    ];
    for a in actions {
        actor.inject_secure_action(a);
    }
}
