//! Translation of `shell/api/actions/creature/creature.go`.
//!
//! Registers every creature lifecycle action against the actor and translates
//! the Go bodies one-to-one. The only deliberate gap is the production
//! Firebase-Auth path inside `/creatures/login`: the Rust workspace does not
//! wire a Firebase SDK, so this port mirrors the Go DEV-mode fallback only
//! (treat `emailToken` as the raw email, or fall back to `username@dev.local`
//! if blank). Wiring the real Firebase verifier is a follow-up.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use base64::Engine;
use chrono::Utc;
use serde_json::{json, Map, Value};

use crate::abstractions::models::action::action::ISecureAction;
use crate::abstractions::models::core::ICore;
use crate::abstractions::models::input::IInput;
use crate::abstractions::models::trx::object_to_map;
use crate::abstractions::state::IState;
use crate::core::module::actor::model::base::info::Info as BaseInfo;
use crate::core::module::actor::model::secured::guard::Guard;
use crate::core::module::actor::model::state::state::State as ActorState;
use crate::shell::api::inputs::creatures::{
    CreateInput as CreatureCreateInput, SignalInput as CreatureSignalInput,
};
use crate::shell::api::inputs::users::{
    AuthenticateInput, CheckSignInput, ConsumeLockInput, DeleteInput, FindInput, GetByUsernameInput,
    GetInput, ListInput, LockTokenInput, LoginInput, MetaInput, MintInput, TransferInput,
    UpdateInput,
};
use crate::shell::api::model::{Creature, Machine, Session, Store, User};
use crate::shell::api::outputs::users::{AuthenticateOutput, GetOutput, LoginOutput};
use crate::shell::api::updates::stores::Send as StoresSend;
use crate::shell::utils::crypto::{secure_key_pairs, secure_unique_string};
use crate::shell::utils::future::async_once;

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

fn as_i64(raw: &Value) -> Option<i64> {
    match raw {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        _ => None,
    }
}

/// Build the `/creatures/create` action.
fn create(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    let app_for_handler = app.clone();
    build_secure_action::<CreatureCreateInput, _>(
        app,
        "/creatures/create",
        anon_guard(),
        move |state: Arc<dyn IState>, input: CreatureCreateInput| -> Result<Value> {
            let trx = state.trx();
            let creature_type = input.typ.clone();
            let username = format!("{}@{}", input.username, state.source());
            let mut chain_id = "main".to_string();
            let mut subchain_id = "main".to_string();
            let mut owner_id = "free".to_string();
            if let Some(s) = input.chain_id.as_ref() {
                if !s.is_empty() {
                    chain_id = s.clone();
                }
            }
            if let Some(s) = input.subchain_id.as_ref() {
                if !s.is_empty() {
                    subchain_id = s.clone();
                }
            }
            if let Some(s) = input.owner_id.as_ref() {
                if !s.is_empty() {
                    owner_id = s.clone();
                }
            }
            if creature_type == "human" {
                chain_id = "main".to_string();
                subchain_id = "main".to_string();
                owner_id = "free".to_string();
            } else if owner_id == "free" {
                owner_id = state.info().user_id();
            }
            if trx.has_index("Creature", "username", "id", &username) {
                return Err(anyhow!("creature username already exists"));
            }
            let balance: i64 = if creature_type == "human" {
                1_000_000_000_000_000
            } else {
                0
            };
            let creature = Creature {
                id: app_for_handler
                    .tools()
                    .storage()
                    .gen_id(&*trx, &input.origin()),
                type_name: creature_type.clone(),
                username: username.clone(),
                public_key: input.public_key.clone(),
                chain_id,
                subchain_id,
                owner_id: owner_id.clone(),
                balance,
                ..Default::default()
            };
            creature.push(&*trx);
            // Mirror to the User table so older code paths (security, signaler,
            // guards) that read obj::User::{id}::{col} continue to work.
            User {
                id: creature.id.clone(),
                typ: creature.type_name.clone(),
                username: creature.username.clone(),
                public_key: creature.public_key.clone(),
                balance: creature.balance,
            }
            .push(&*trx);
            // Machine-type creatures double as "machines" in the program / Deploy
            // API which keys off the Machine table.
            if creature_type == "machine" {
                Machine {
                    id: creature.id.clone(),
                    chain_id: creature.chain_id.clone(),
                    owner_id: creature.owner_id.clone(),
                    username: creature.username.clone(),
                    ..Default::default()
                }
                .push(&*trx);
            }
            let session = Session {
                id: app_for_handler
                    .tools()
                    .storage()
                    .gen_id(&*trx, &input.origin()),
                user_id: creature.id.clone(),
            };
            session.push(&*trx);
            let _ = trx.put_json(
                &format!("CreatMeta::{}", creature.id),
                "metadata",
                &input.metadata,
                false,
            );
            let _ = trx.put_json(
                &format!("UserMeta::{}", creature.id),
                "metadata",
                &input.metadata,
                false,
            );
            if creature_type != "human" {
                trx.put_link(&format!("ownerof::{}::{}", owner_id, creature.id), "true");
            }
            Ok(json!({"creature": creature, "session": session}))
        },
    )
}

fn get(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<GetInput, _>(
        app,
        "/creatures/get",
        user_guard(),
        move |state: Arc<dyn IState>, input: GetInput| -> Result<Value> {
            let trx = state.trx();
            if trx.has_obj("Creature", &input.user_id) {
                let creature = Creature {
                    id: input.user_id.clone(),
                    ..Default::default()
                }
                .pull(&*trx);
                return Ok(json!({"creature": creature}));
            }
            Err(anyhow!("creature not found"))
        },
    )
}

fn list(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<ListInput, _>(
        app,
        "/creatures/list",
        user_guard(),
        move |state: Arc<dyn IState>, input: ListInput| -> Result<Value> {
            let creatures = Creature::all(&*state.trx(), input.offset, input.count)?;
            Ok(json!({"creatures": creatures}))
        },
    )
}

fn transfer(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<TransferInput, _>(
        app,
        "/creatures/transfer",
        user_guard(),
        move |state: Arc<dyn IState>, input: TransferInput| -> Result<Value> {
            let trx = state.trx();
            let mut from = Creature {
                id: state.info().user_id(),
                ..Default::default()
            }
            .pull(&*trx);
            if from.id.is_empty() {
                return Err(anyhow!("sender creature not found"));
            }
            if from.balance < input.amount {
                return Err(anyhow!("your balance is not enough"));
            }
            let to_id = trx.get_index("Creature", "username", "id", &input.to_username);
            if to_id.is_empty() {
                return Err(anyhow!("target creature not found"));
            }
            let mut to = Creature {
                id: to_id,
                ..Default::default()
            }
            .pull(&*trx);
            if to.id.is_empty() {
                return Err(anyhow!("target creature not found"));
            }
            from.balance -= input.amount;
            to.balance += input.amount;
            from.push(&*trx);
            to.push(&*trx);
            Ok(json!({}))
        },
    )
}

fn signal(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    let app_for_handler = app.clone();
    build_secure_action::<CreatureSignalInput, _>(
        app,
        "/creatures/signal",
        user_guard(),
        move |state: Arc<dyn IState>, input: CreatureSignalInput| -> Result<Value> {
            let trx = state.trx();
            let sender_creature = Creature {
                id: state.info().user_id(),
                ..Default::default()
            }
            .pull(&*trx);
            let sender = User {
                id: sender_creature.id.clone(),
                typ: sender_creature.type_name.clone(),
                username: sender_creature.username.clone(),
                public_key: sender_creature.public_key.clone(),
                balance: 0,
            };
            let store_id = state.info().store_id();
            if input.typ == "all" {
                if store_id.is_empty() {
                    return Err(anyhow!("storeId is required for broadcast"));
                }
                if trx.get_link(&format!("onaccess::{}::{}", store_id, state.info().user_id()))
                    != "true"
                {
                    return Err(anyhow!("access denied"));
                }
                let packet = StoresSend {
                    action: "broadcast".to_string(),
                    user: sender.clone(),
                    data: input.data.clone(),
                    is_temp: input.temp,
                    ..Default::default()
                };
                let app_async = app_for_handler.clone();
                let store_id_async = store_id.clone();
                let exception_user_id = state.info().user_id();
                let _ = async_once(move || {
                    app_async.tools().signaler().signal_group(
                        "creatures/signal",
                        &store_id_async,
                        serde_json::to_value(&packet).unwrap_or(Value::Null),
                        true,
                        vec![exception_user_id],
                    );
                });
                return Ok(json!({"passed": true}));
            }
            if input.typ != "pvp" {
                return Err(anyhow!("unknown signal type"));
            }
            if input.creature_id.is_empty() {
                return Err(anyhow!("creatureId is required for pvp"));
            }
            let target_id = if !input.program_id.is_empty() {
                input.program_id.clone()
            } else {
                input.creature_id.clone()
            };
            let packet = StoresSend {
                action: "single".to_string(),
                user: sender,
                data: input.data.clone(),
                is_temp: input.temp,
                entity_id: input.entity_id.clone(),
                ..Default::default()
            };
            let app_async = app_for_handler.clone();
            let _ = async_once(move || {
                app_async.tools().signaler().signal_user(
                    "creatures/signal",
                    &target_id,
                    serde_json::to_value(&packet).unwrap_or(Value::Null),
                    true,
                );
            });
            Ok(json!({"passed": true}))
        },
    )
}

fn authenticate(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    let app_for_handler = app.clone();
    build_secure_action::<AuthenticateInput, _>(
        app,
        "/creatures/authenticate",
        user_guard(),
        move |state: Arc<dyn IState>, _: AuthenticateInput| -> Result<Value> {
            let user_id = state.info().user_id();
            // Re-enter /creatures/get on the same trx with the caller as
            // the target user. Mirrors how the Go path stitched together a
            // fresh State+Info pair off mainstate.NewState.
            let inner_info = Arc::new(BaseInfo::new("", ""));
            let inner_state: Arc<dyn IState> =
                Arc::new(ActorState::new(Some(inner_info), Some(state.trx()), ""));
            let get_action = app_for_handler
                .actor()
                .fetch_action("/creatures/get")
                .ok_or_else(|| anyhow!("/creatures/get not registered"))?;
            let typed_input: Arc<dyn IInput> = Arc::new(GetInput {
                user_id: user_id.clone(),
            });
            let (_code, res) = get_action.act(inner_state, typed_input)?;
            let creature: Creature = res
                .get("creature")
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default();
            let mut user_map: HashMap<String, Value> = HashMap::new();
            user_map.insert("id".to_string(), json!(creature.id));
            user_map.insert("type".to_string(), json!(creature.type_name));
            user_map.insert("username".to_string(), json!(creature.username));
            user_map.insert("publicKey".to_string(), json!(creature.public_key));
            user_map.insert("balance".to_string(), json!(creature.balance));
            Ok(serde_json::to_value(AuthenticateOutput {
                authenticated: true,
                user: user_map,
            })?)
        },
    )
}

fn mint(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<MintInput, _>(
        app,
        "/creatures/mint",
        user_guard(),
        move |state: Arc<dyn IState>, input: MintInput| -> Result<Value> {
            if state.info().user_id() != "1@global" {
                return Err(anyhow!("access denied"));
            }
            let trx = state.trx();
            let username = User {
                id: trx.get_link(&format!("UserEmailToId::{}", input.to_user_email)),
                ..Default::default()
            }
            .pull(&*trx)
            .username;
            let to_user_id = trx.get_index("User", "username", "id", &username);
            if to_user_id.is_empty() {
                return Err(anyhow!("target user not found"));
            }
            let mut to_user = User {
                id: to_user_id,
                ..Default::default()
            }
            .pull(&*trx);
            to_user.balance += input.amount;
            to_user.push(&*trx);
            Creature {
                id: to_user.id.clone(),
                type_name: to_user.typ.clone(),
                username: to_user.username.clone(),
                public_key: to_user.public_key.clone(),
                chain_id: "main".to_string(),
                subchain_id: "main".to_string(),
                owner_id: "free".to_string(),
                balance: to_user.balance,
                ..Default::default()
            }
            .push(&*trx);
            Ok(json!({}))
        },
    )
}

fn check_sign(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    let app_for_handler = app.clone();
    build_secure_action::<CheckSignInput, _>(
        app,
        "/creatures/checkSign",
        user_guard(),
        move |state: Arc<dyn IState>, input: CheckSignInput| -> Result<Value> {
            if state.info().user_id() != "1@global" {
                return Err(anyhow!("access denied"));
            }
            let data = match base64::engine::general_purpose::STANDARD.decode(&input.payload) {
                Ok(d) => d,
                Err(e) => {
                    log::warn!("checkSign decode: {}", e);
                    return Ok(json!({"valid": false}));
                }
            };
            let (success, _, _) = app_for_handler.tools().security().auth_with_signature(
                &input.user_id,
                &data,
                &input.signature,
            );
            if success {
                let email = state
                    .trx()
                    .get_link(&format!("UserIdToEmail::{}", input.user_id));
                return Ok(json!({"valid": true, "email": email}));
            }
            Ok(json!({"valid": false}))
        },
    )
}

fn lock_token(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<LockTokenInput, _>(
        app,
        "/creatures/lockToken",
        user_guard(),
        move |state: Arc<dyn IState>, input: LockTokenInput| -> Result<Value> {
            let trx = state.trx();
            let mut user = User {
                id: state.info().user_id(),
                ..Default::default()
            }
            .pull(&*trx);

            let mut steps: Vec<Value> = Vec::with_capacity(input.steps.len().max(1));
            if !input.steps.is_empty() {
                for (i, step) in input.steps.iter().enumerate() {
                    if step.amount <= 0 {
                        return Err(anyhow!("step {} amount must be greater than zero", i));
                    }
                    if step.unlock_at <= 0 {
                        return Err(anyhow!(
                            "step {} unlockAt must be a unix timestamp in milliseconds",
                            i
                        ));
                    }
                    steps.push(json!({
                        "amount": step.amount,
                        "unlockAt": step.unlock_at,
                        "consumed": false,
                    }));
                }
            } else {
                if input.amount <= 0 {
                    return Err(anyhow!("amount must be greater than zero"));
                }
                if input.unlock_at <= 0 {
                    return Err(anyhow!("unlockAt must be a unix timestamp in milliseconds"));
                }
                steps.push(json!({
                    "amount": input.amount,
                    "unlockAt": input.unlock_at,
                    "consumed": false,
                }));
            }

            let total_amount: i64 = steps
                .iter()
                .map(|s| s.get("amount").and_then(|v| v.as_i64()).unwrap_or(0))
                .sum();
            if user.balance < total_amount {
                return Err(anyhow!("your balance is not enough"));
            }
            let lock_id = secure_unique_string();
            if input.typ == "pay" {
                if !trx.has_obj("User", &input.target) {
                    return Err(anyhow!("target user not acceptable"));
                }
                user.balance -= total_amount;
                user.push(&*trx);
                let payload = json!({
                    "type": "pay",
                    "amount": total_amount,
                    "remainingAmount": total_amount,
                    "userId": input.target,
                    "steps": steps,
                });
                trx.put_json(
                    &format!("Json::User::{}", state.info().user_id()),
                    &format!("lockedTokens.{}", lock_id),
                    &payload,
                    true,
                )?;
            } else {
                return Err(anyhow!("unknown lock type"));
            }
            Ok(json!({"tokenId": lock_id}))
        },
    )
}

fn consume_lock(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    let app_for_handler = app.clone();
    build_secure_action::<ConsumeLockInput, _>(
        app,
        "/creatures/consumeLock",
        user_guard(),
        move |state: Arc<dyn IState>, input: ConsumeLockInput| -> Result<Value> {
            let trx = state.trx();
            let mut receiver = User {
                id: state.info().user_id(),
                ..Default::default()
            }
            .pull(&*trx);
            if input.typ != "pay" {
                return Err(anyhow!("unknown lock type"));
            }
            if !trx.has_obj("User", &input.user_id) {
                return Err(anyhow!("payer user not found"));
            }
            let sender = User {
                id: input.user_id.clone(),
                ..Default::default()
            }
            .pull(&*trx);
            let payment_map = match trx.get_json(
                &format!("Json::User::{}", sender.id),
                &format!("lockedTokens.{}", input.lock_id),
            ) {
                Ok(m) => m,
                Err(_) => return Err(anyhow!("lock not found")),
            };
            let mut payment: Map<String, Value> = payment_map;
            let steps_raw = match payment.get("steps") {
                Some(Value::Array(arr)) if !arr.is_empty() => arr.clone(),
                _ => return Err(anyhow!("lock does not include steps")),
            };
            let mut step_index: i64 = input.step.unwrap_or(-1);
            let now = Utc::now().timestamp_millis();
            let mut parsed_steps: Vec<Map<String, Value>> = Vec::with_capacity(steps_raw.len());
            let mut parsed_amounts: Vec<i64> = Vec::with_capacity(steps_raw.len());
            let mut parsed_unlocks: Vec<i64> = Vec::with_capacity(steps_raw.len());
            for raw_step in steps_raw.iter() {
                let step_map = match raw_step {
                    Value::Object(o) => o.clone(),
                    _ => return Err(anyhow!("invalid lock step")),
                };
                let step_amount = step_map.get("amount").and_then(as_i64).unwrap_or(0);
                if step_amount <= 0 {
                    return Err(anyhow!("invalid lock step amount"));
                }
                let unlock_at = step_map.get("unlockAt").and_then(as_i64).unwrap_or(0);
                if unlock_at <= 0 {
                    return Err(anyhow!("invalid lock step unlockAt"));
                }
                let consumed = step_map
                    .get("consumed")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                parsed_steps.push(step_map);
                parsed_amounts.push(step_amount);
                parsed_unlocks.push(unlock_at);
                if step_index == -1 && !consumed && now >= unlock_at && step_amount == input.amount
                {
                    step_index = (parsed_steps.len() - 1) as i64;
                }
            }
            if step_index < 0 || (step_index as usize) >= parsed_steps.len() {
                return Err(anyhow!("lock step not found"));
            }
            let idx = step_index as usize;
            let selected_step = &mut parsed_steps[idx];
            let selected_amount = parsed_amounts[idx];
            let selected_unlock_at = parsed_unlocks[idx];
            if now < selected_unlock_at {
                return Err(anyhow!("lock step is not consumable yet"));
            }
            if selected_step
                .get("consumed")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                return Err(anyhow!("lock step already consumed"));
            }
            if input.amount != selected_amount {
                return Err(anyhow!("amount of payment not matched"));
            }
            let sign_payload = format!(
                "{}:{}:{}:{}:{}",
                input.lock_id, idx, selected_unlock_at, selected_amount, receiver.id
            );
            let (success, _, _) = app_for_handler.tools().security().auth_with_signature(
                &input.user_id,
                sign_payload.as_bytes(),
                &input.signature,
            );
            if !success {
                return Err(anyhow!("signature not verified"));
            }
            let typ = payment.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if typ != "pay" {
                return Err(anyhow!("type is not payment"));
            }
            let target = payment.get("userId").and_then(|v| v.as_str()).unwrap_or("");
            if target != receiver.id {
                return Err(anyhow!("you are not target"));
            }
            selected_step.insert("consumed".to_string(), Value::Bool(true));
            selected_step.insert("consumedAt".to_string(), json!(now));
            receiver.balance += input.amount;
            receiver.push(&*trx);
            let mut remaining_amount: i64 = 0;
            for (i, step_map) in parsed_steps.iter().enumerate() {
                let consumed = step_map
                    .get("consumed")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if !consumed {
                    remaining_amount += parsed_amounts[i];
                }
            }
            if remaining_amount == 0 {
                trx.del_json(
                    &format!("Json::User::{}", sender.id),
                    &format!("lockedTokens.{}", input.lock_id),
                );
            } else {
                let total_amount = payment.get("amount").and_then(as_i64).unwrap_or(0);
                if total_amount <= 0 {
                    return Err(anyhow!("invalid lock total amount"));
                }
                let steps_value: Value = Value::Array(
                    parsed_steps
                        .iter()
                        .map(|m| Value::Object(m.clone()))
                        .collect(),
                );
                payment.insert("steps".to_string(), steps_value);
                payment.insert("remainingAmount".to_string(), json!(remaining_amount));
                payment.insert(
                    "consumedAmount".to_string(),
                    json!(total_amount - remaining_amount),
                );
                trx.put_json(
                    &format!("Json::User::{}", sender.id),
                    &format!("lockedTokens.{}", input.lock_id),
                    &Value::Object(payment),
                    true,
                )?;
            }
            Ok(json!({
                "success": true,
                "step": idx,
                "remainingAmount": remaining_amount,
            }))
        },
    )
}

fn login(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    let app_for_handler = app.clone();
    build_secure_action::<LoginInput, _>(
        app,
        "/creatures/login",
        anon_guard(),
        move |state: Arc<dyn IState>, input: LoginInput| -> Result<Value> {
            // DEV-mode Firebase-Auth fallback. The Go module optionally
            // verified the supplied emailToken with the Firebase Admin SDK;
            // the Rust workspace doesn't carry a Firebase dependency so this
            // port short-circuits straight to the DEV path. Treat the token as
            // the raw email or fall back to a synthetic `username@dev.local`.
            let mut email = input.email_token.trim().to_string();
            if email.is_empty() || !email.contains('@') {
                email = format!("{}@dev.local", input.username);
            }
            log::info!("[DEV] firebase disabled; accepting login for email: {}", email);

            let trx = state.trx();
            let user_id = trx.get_link(&format!("UserEmailToId::{}", email));
            if !user_id.is_empty() {
                let user = User {
                    id: user_id.clone(),
                    ..Default::default()
                }
                .pull(&*trx);
                let session_id = trx.get_index("Session", "userId", "id", &user.id);
                let session = Session {
                    id: session_id,
                    ..Default::default()
                }
                .pull(&*trx);
                let private_key = trx.get_link(&format!("UserPrivateKey::{}", user.id));
                return Ok(serde_json::to_value(LoginOutput {
                    user,
                    session,
                    private_key,
                })?);
            }
            let expected_username = format!("{}@{}", input.username, app_for_handler.id());
            if trx.has_index("User", "username", "id", &expected_username) {
                return Err(anyhow!("username already exist"));
            }
            let (priv_raw, pub_raw) = secure_key_pairs("")?;
            let priv_key = String::from_utf8_lossy(&priv_raw).into_owned();
            let pub_key = String::from_utf8_lossy(&pub_raw).into_owned();
            let create_input = CreatureCreateInput {
                typ: "human".to_string(),
                username: input.username.clone(),
                public_key: pub_key,
                metadata: input.metadata.clone(),
                ..Default::default()
            };
            // Call /creatures/create's action directly on the current state.
            // Going through the secured chain re-submits the request and
            // deadlocks the chain processor (single-threaded), exactly like
            // the Go side.
            let create_action = app_for_handler
                .actor()
                .fetch_action("/creatures/create")
                .ok_or_else(|| anyhow!("/creatures/create not registered"))?;
            let typed_input: Arc<dyn IInput> = Arc::new(create_input);
            let (_code, res) = create_action.act(state.clone(), typed_input)?;
            let creature: Creature = res
                .get("creature")
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default();
            let session: Session = res
                .get("session")
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default();
            let user_val = User {
                id: creature.id.clone(),
                typ: creature.type_name.clone(),
                username: creature.username.clone(),
                public_key: creature.public_key.clone(),
                balance: creature.balance,
            };
            trx.put_link(&format!("UserPrivateKey::{}", user_val.id), &priv_key);
            trx.put_link(&format!("UserEmailToId::{}", email), &user_val.id);
            trx.put_link(&format!("UserIdToEmail::{}", user_val.id), &email);
            Ok(serde_json::to_value(LoginOutput {
                user: user_val,
                session,
                private_key: priv_key,
            })?)
        },
    )
}

fn delete(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<DeleteInput, _>(
        app,
        "/creatures/delete",
        user_guard(),
        move |state: Arc<dyn IState>, input: DeleteInput| -> Result<Value> {
            if input.user_id != state.info().user_id() && state.info().user_id() != "1@global" {
                return Err(anyhow!("access denied"));
            }
            let trx = state.trx();
            let user = User {
                id: input.user_id.clone(),
                ..Default::default()
            }
            .pull(&*trx);
            if user.id.is_empty() {
                return Err(anyhow!("user not found"));
            }
            user.delete(&*trx);
            trx.del_json(&format!("UserMeta::{}", input.user_id), "metadata");
            let store_list = Store::list(
                &*trx,
                &format!("hasaccess::{}::", input.user_id),
                false,
                &HashMap::new(),
                &HashMap::new(),
                -1,
                -1,
            )
            .unwrap_or_default();
            for store in &store_list {
                trx.del_key(&format!(
                    "link::onaccess::{}::{}",
                    store.id, input.user_id
                ));
            }
            let created_store_list = Store::list(
                &*trx,
                &format!("creatorof::{}::", input.user_id),
                false,
                &HashMap::new(),
                &HashMap::new(),
                -1,
                -1,
            )
            .unwrap_or_default();
            for store in &created_store_list {
                store.delete(&*trx);
            }
            trx.del_key(&format!("link::UserIdToEmail::{}", input.user_id));
            let email = trx.get_link(&format!("UserIdToEmail::{}", input.user_id));
            trx.del_key(&format!("link::UserEmailToId::{}", email));
            trx.del_key(&format!("link::UserPrivateKey::{}", input.user_id));
            Ok(json!({}))
        },
    )
}

fn update(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<UpdateInput, _>(
        app,
        "/creatures/update",
        user_guard(),
        move |state: Arc<dyn IState>, input: UpdateInput| -> Result<Value> {
            if input.user_id != state.info().user_id() && state.info().user_id() != "1@global" {
                return Err(anyhow!("access denied"));
            }
            let trx = state.trx();
            let mut user = User {
                id: input.user_id.clone(),
                ..Default::default()
            }
            .pull(&*trx);
            if user.id.is_empty() {
                return Err(anyhow!("user not found"));
            }
            if let Some(pk) = input.public_key.as_ref() {
                user.public_key = pk.clone();
            }
            if let Some(t) = input.typ.as_ref() {
                user.typ = t.clone();
            }
            if let Some(name) = input.username.as_ref() {
                let base_username = user.username.split('@').next().unwrap_or("").to_string();
                if *name != base_username {
                    let next_username = format!("{}@{}", name, state.source());
                    if trx.has_index("User", "username", "id", &next_username) {
                        return Err(anyhow!("username already exists"));
                    }
                    trx.del_key(&format!(
                        "index::User::username::id::{}",
                        user.username
                    ));
                    user.username = next_username;
                }
            }
            user.push(&*trx);
            Creature {
                id: user.id.clone(),
                type_name: user.typ.clone(),
                username: user.username.clone(),
                public_key: user.public_key.clone(),
                chain_id: "main".to_string(),
                subchain_id: "main".to_string(),
                owner_id: "free".to_string(),
                balance: user.balance,
                ..Default::default()
            }
            .push(&*trx);
            Ok(json!({}))
        },
    )
}

fn meta(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<MetaInput, _>(
        app,
        "/creatures/meta",
        user_guard(),
        move |state: Arc<dyn IState>, input: MetaInput| -> Result<Value> {
            let trx = state.trx();
            let user = User {
                id: input.user_id.clone(),
                ..Default::default()
            }
            .pull(&*trx);
            if user.id.is_empty() {
                return Err(anyhow!("user not found"));
            }
            let m = trx
                .get_json(&format!("UserMeta::{}", input.user_id), "metadata")
                .unwrap_or_default();
            Ok(Value::Object(m))
        },
    )
}

fn get_by_username(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<GetByUsernameInput, _>(
        app,
        "/creatures/getByUsername",
        user_guard(),
        move |state: Arc<dyn IState>, input: GetByUsernameInput| -> Result<Value> {
            let trx = state.trx();
            let user_id = trx.get_index("User", "username", "id", &input.username);
            if user_id.is_empty() {
                return Err(anyhow!("user not found"));
            }
            let result = User {
                id: user_id,
                ..Default::default()
            }
            .pull(&*trx);
            let m = object_to_map(&result).unwrap_or_default();
            // NOTE: Go ran the registered `modelExtender["user"]` getters here
            // to inject dynamic fields. The Rust shell does not yet thread
            // ExtendedField through the per-action handlers, so we mirror the
            // baseline object only — extender support is a follow-up.
            let user_map: HashMap<String, Value> = m.into_iter().collect();
            Ok(serde_json::to_value(GetOutput { user: user_map })?)
        },
    )
}

fn find(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<FindInput, _>(
        app,
        "/creatures/find",
        user_guard(),
        move |state: Arc<dyn IState>, input: FindInput| -> Result<Value> {
            let trx = state.trx();
            let users = User::search(
                &*trx,
                0,
                1,
                "username",
                &input.username,
                &HashMap::new(),
            )
            .unwrap_or_default();
            if users.is_empty() {
                return Err(anyhow!("user not found"));
            }
            let result = users.into_iter().next().unwrap();
            let m = object_to_map(&result).unwrap_or_default();
            let user_map: HashMap<String, Value> = m.into_iter().collect();
            Ok(serde_json::to_value(GetOutput { user: user_map })?)
        },
    )
}

/// Install every creature action onto the actor.
pub fn install(app: Arc<dyn ICore>) {
    let actor = app.actor();
    let handlers: Vec<Arc<dyn ISecureAction>> = vec![
        create(app.clone()),
        get(app.clone()),
        list(app.clone()),
        transfer(app.clone()),
        signal(app.clone()),
        authenticate(app.clone()),
        mint(app.clone()),
        check_sign(app.clone()),
        lock_token(app.clone()),
        consume_lock(app.clone()),
        login(app.clone()),
        delete(app.clone()),
        update(app.clone()),
        meta(app.clone()),
        get_by_username(app.clone()),
        find(app.clone()),
    ];
    for h in handlers {
        actor.inject_secure_action(h);
    }
}
