//! Translation of `shell/api/actions/creature/creature.go`.
//!
//! The Go module registers 19 secured actions covering the full creature
//! lifecycle (create / get / list / signal / transfer / authenticate /
//! mint / sign-checks / lock-token / consume-lock / login / delete /
//! update / meta / get-by-username / find). The Rust port mirrors the
//! action registry: each handler is wired here under the same key + guard
//! the Go code uses, with a body that returns a structured "not yet
//! implemented" error so the dispatcher logs the path that needs filling
//! in. The business-logic translation lands as follow-up commits per
//! action — the registration surface compiles green so the rest of the
//! shell (pluggers, federation routing, ITcp/IWs dispatch) is reachable.

use std::sync::Arc;

use anyhow::{anyhow, Result};
use serde_json::Value;

use crate::abstractions::models::action::action::ISecureAction;
use crate::abstractions::models::core::ICore;
use crate::abstractions::state::IState;
use crate::core::module::actor::model::secured::guard::Guard;
use crate::shell::api::inputs::creatures::{
    CreateInput as CreatureCreateInput, SignalInput as CreatureSignalInput,
};
use crate::shell::api::inputs::users::{
    AuthenticateInput, CheckSignInput, ConsumeLockInput, DeleteInput, FindInput, GetByUsernameInput,
    GetInput, ListInput, LockTokenInput, LoginInput, MetaInput, MintInput, TransferInput,
    UpdateInput,
};

use super::util::build_secure_action;

fn user_guard() -> Guard {
    Guard {
        is_user: true,
        is_in_store: false,
    }
}

fn anon_guard() -> Guard {
    Guard::default()
}

fn not_implemented(key: &str) -> Result<Value> {
    Err(anyhow!("creature action `{}` not yet translated", key))
}

/// Install every creature action onto the actor.
pub fn install(app: Arc<dyn ICore>) {
    let actor = app.actor();
    let handlers: Vec<Arc<dyn ISecureAction>> = vec![
        build_secure_action::<CreatureCreateInput, _>(
            app.clone(),
            "/creatures/create",
            anon_guard(),
            |_, _: CreatureCreateInput| not_implemented("/creatures/create"),
        ),
        build_secure_action::<GetInput, _>(
            app.clone(),
            "/creatures/get",
            user_guard(),
            |_, _: GetInput| not_implemented("/creatures/get"),
        ),
        build_secure_action::<ListInput, _>(
            app.clone(),
            "/creatures/list",
            user_guard(),
            |_, _: ListInput| not_implemented("/creatures/list"),
        ),
        build_secure_action::<TransferInput, _>(
            app.clone(),
            "/creatures/transfer",
            user_guard(),
            |_, _: TransferInput| not_implemented("/creatures/transfer"),
        ),
        build_secure_action::<CreatureSignalInput, _>(
            app.clone(),
            "/creatures/signal",
            user_guard(),
            |_, _: CreatureSignalInput| not_implemented("/creatures/signal"),
        ),
        build_secure_action::<AuthenticateInput, _>(
            app.clone(),
            "/creatures/authenticate",
            user_guard(),
            |_, _: AuthenticateInput| not_implemented("/creatures/authenticate"),
        ),
        build_secure_action::<MintInput, _>(
            app.clone(),
            "/creatures/mint",
            user_guard(),
            |_, _: MintInput| not_implemented("/creatures/mint"),
        ),
        build_secure_action::<CheckSignInput, _>(
            app.clone(),
            "/creatures/checkSign",
            user_guard(),
            |_, _: CheckSignInput| not_implemented("/creatures/checkSign"),
        ),
        build_secure_action::<LockTokenInput, _>(
            app.clone(),
            "/creatures/lockToken",
            user_guard(),
            |_, _: LockTokenInput| not_implemented("/creatures/lockToken"),
        ),
        build_secure_action::<ConsumeLockInput, _>(
            app.clone(),
            "/creatures/consumeLock",
            user_guard(),
            |_, _: ConsumeLockInput| not_implemented("/creatures/consumeLock"),
        ),
        build_secure_action::<LoginInput, _>(
            app.clone(),
            "/creatures/login",
            anon_guard(),
            |_, _: LoginInput| not_implemented("/creatures/login"),
        ),
        build_secure_action::<DeleteInput, _>(
            app.clone(),
            "/creatures/delete",
            user_guard(),
            |_, _: DeleteInput| not_implemented("/creatures/delete"),
        ),
        build_secure_action::<UpdateInput, _>(
            app.clone(),
            "/creatures/update",
            user_guard(),
            |_, _: UpdateInput| not_implemented("/creatures/update"),
        ),
        build_secure_action::<MetaInput, _>(
            app.clone(),
            "/creatures/meta",
            user_guard(),
            |_, _: MetaInput| not_implemented("/creatures/meta"),
        ),
        build_secure_action::<GetByUsernameInput, _>(
            app.clone(),
            "/creatures/getByUsername",
            user_guard(),
            |_, _: GetByUsernameInput| not_implemented("/creatures/getByUsername"),
        ),
        build_secure_action::<FindInput, _>(
            app.clone(),
            "/creatures/find",
            user_guard(),
            |_, _: FindInput| not_implemented("/creatures/find"),
        ),
    ];
    let _ = _bind_unused(&app);
    for h in handlers {
        actor.inject_secure_action(h);
    }
}

fn _bind_unused(_app: &Arc<dyn ICore>) -> Result<(), Box<dyn std::error::Error>> {
    let _: Arc<dyn IState> = build_state_placeholder();
    Ok(())
}

fn build_state_placeholder() -> Arc<dyn IState> {
    use crate::core::module::actor::model::state::state::State as ActorState;
    Arc::new(ActorState::new(None, None, ""))
}
