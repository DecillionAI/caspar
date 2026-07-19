//! Proxy entities — non-runnable creature program entities that forward
//! signals.
//!
//! A developer can deploy an entity with `entityType: "proxy"`. Such an
//! entity has no runnable module; it only holds a data file plus a target
//! descriptor (another program's entity, possibly of another creature). When
//! the proxy entity receives a signal, the node:
//!
//! 1. attaches the stored data file's content to the received payload
//!    (under the configured attach field),
//! 2. stamps a correlation id on the packet and records who sent it,
//! 3. forwards the repackaged signal to the target entity.
//!
//! When the target eventually signals back carrying the same correlation id
//! (addressed to the proxy's program), the node resolves the recorded
//! correlation and delivers the response to the original sender — with the
//! proxy entity's identity as the response sender, so the requester only
//! ever observes the proxy.
//!
//! This is the substrate for "agent" deployments: an AI-agent skill file is
//! deployed as a proxy entity targeting a davinci docker-VM creature; every
//! request through the proxy reaches davinci with the skill attached (used
//! as the session's system instruction) and davinci's final result flows
//! back through the proxy to the requester.

use std::fs;
use std::sync::{Arc, Mutex};

use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::models::core::ICore;
use crate::models::transaction::ITrx;
use crate::shell::api::model::{Machine, Program, User};
use crate::shell::api::packets::stores::Send as StoresSend;

/// The pseudo-runtime key a proxy entity is deployed under. It is not a VM
/// runtime: nothing ever runs for a proxy entity.
pub const PROXY_RUNTIME_KEY: &str = "proxy";

/// Default payload field the proxy's data file content is attached under.
pub const DEFAULT_ATTACH_FIELD: &str = "attachment";

fn correlation_key(correlation_id: &str) -> String {
    format!("Json::ProxyCorrelation::{}", correlation_id)
}

fn config_key(program_id: &str, entity_id: &str) -> String {
    format!("Json::ProxyEntity::{}::{}", program_id, entity_id)
}

/// Normalized proxy-entity configuration.
#[derive(Debug, Clone, Default)]
pub struct ProxyConfig {
    pub target_program_id: String,
    pub target_entity_id: String,
    pub attach_field: String,
}

impl ProxyConfig {
    pub fn to_value(&self) -> Value {
        json!({
            "targetProgramId": self.target_program_id,
            "targetEntityId": self.target_entity_id,
            "attachField": self.attach_field,
        })
    }

    pub fn from_value(v: &Value) -> ProxyConfig {
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let mut cfg = ProxyConfig {
            target_program_id: s("targetProgramId"),
            target_entity_id: s("targetEntityId"),
            attach_field: s("attachField"),
        };
        if cfg.attach_field.is_empty() {
            cfg.attach_field = DEFAULT_ATTACH_FIELD.to_string();
        }
        cfg
    }
}

/// Extract the proxy configuration from a deploy request's metadata. Accepts
/// either a nested `"proxy": {...}` object or flat `proxyTarget*` keys.
pub fn config_from_metadata<M>(get: M) -> Result<ProxyConfig, String>
where
    M: Fn(&str) -> Option<Value>,
{
    let nested = get("proxy").unwrap_or(Value::Null);
    let pick = |nested_key: &str, flat_key: &str| -> String {
        nested
            .get(nested_key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|s| !s.is_empty())
            .or_else(|| {
                get(flat_key)
                    .and_then(|v| v.as_str().map(str::to_string))
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or_default()
    };
    let mut target_program_id = pick("targetProgramId", "proxyTargetProgramId");
    if target_program_id.is_empty() {
        target_program_id = pick("targetMachineId", "proxyTargetMachineId");
    }
    if target_program_id.is_empty() {
        target_program_id = pick("targetCreatureId", "proxyTargetCreatureId");
    }
    if target_program_id.is_empty() {
        return Err(
            "proxy entity deploy requires metadata.proxy.targetProgramId (the program/creature the proxy forwards to)"
                .to_string(),
        );
    }
    let target_entity_id = pick("targetEntityId", "proxyTargetEntityId");
    let mut attach_field = pick("attachField", "proxyAttachField");
    if attach_field.is_empty() {
        attach_field = DEFAULT_ATTACH_FIELD.to_string();
    }
    Ok(ProxyConfig {
        target_program_id,
        target_entity_id,
        attach_field,
    })
}

/// Record a deployed proxy entity inside an open state transaction: the
/// entity record itself, the `vmEntityType`/`vmEntityPath` links (path points
/// at the stored data file) and the proxy target configuration.
pub fn record_proxy_entity(
    trx: &dyn ITrx,
    program_id: &str,
    entity_id: &str,
    data_path: &str,
    config: &ProxyConfig,
) {
    crate::shell::api::model::Entity {
        program_id: program_id.to_string(),
        entity_id: entity_id.to_string(),
        entity_type: PROXY_RUNTIME_KEY.to_string(),
        image_name: entity_id.to_string(),
    }
    .push(trx);
    trx.put_link(
        &format!("vmEntityType::{}::{}", program_id, entity_id),
        PROXY_RUNTIME_KEY,
    );
    trx.put_link(
        &format!("vmEntityPath::{}::{}", program_id, entity_id),
        data_path,
    );
    let _ = trx.put_json(
        &config_key(program_id, entity_id),
        "config",
        &config.to_value(),
        true,
    );
}

fn read_state<T, F>(app: &Arc<dyn ICore>, default: T, f: F) -> T
where
    T: Clone + Send + 'static,
    F: Fn(&dyn ITrx) -> T + Send + Sync + 'static,
{
    let slot = Arc::new(Mutex::new(default));
    let slot_clone = slot.clone();
    app.modify_state(
        true,
        Box::new(move |trx: &dyn ITrx| {
            *slot_clone.lock().unwrap() = f(trx);
            Ok(())
        }),
    );
    let out = slot.lock().unwrap().clone();
    out
}

/// The identity a proxied packet travels under: the proxy's program, with the
/// owning machine creature's username when available.
fn proxy_identity(app: &Arc<dyn ICore>, program_id: &str) -> User {
    let program_id_owned = program_id.to_string();
    read_state(app, User::default(), move |trx| {
        let program = Program {
            id: program_id_owned.clone(),
            ..Default::default()
        }
        .pull(trx);
        let machine = Machine {
            id: program.machine_id.clone(),
            ..Default::default()
        }
        .pull(trx, false);
        User {
            id: program_id_owned.clone(),
            typ: "machine".to_string(),
            username: if machine.username.is_empty() {
                program_id_owned.clone()
            } else {
                machine.username
            },
            ..Default::default()
        }
    })
}

/// Pull the correlation id out of an incoming `creatures/signal` payload:
/// either a top-level `correlationId` or one embedded in the packet's `data`
/// JSON string.
fn extract_correlation_id(value: &Value) -> String {
    fn from_obj(v: &Value) -> String {
        v.get("correlationId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    }
    let c = from_obj(value);
    if !c.is_empty() {
        return c;
    }
    if let Some(data) = value.get("data").and_then(Value::as_str) {
        if let Ok(parsed) = serde_json::from_str::<Value>(data) {
            let c = from_obj(&parsed);
            if !c.is_empty() {
                return c;
            }
            // Client convention: the caller's payload travels as a JSON
            // string under `data.payload` — the correlation id the requester
            // wants echoed back lives inside it.
            match parsed.get("payload") {
                Some(Value::String(p)) => {
                    if let Ok(inner) = serde_json::from_str::<Value>(p) {
                        let c = from_obj(&inner);
                        if !c.is_empty() {
                            return c;
                        }
                    }
                }
                Some(obj @ Value::Object(_)) => {
                    let c = from_obj(obj);
                    if !c.is_empty() {
                        return c;
                    }
                }
                _ => {}
            }
        }
    }
    String::new()
}

/// Response direction. If `value` carries a correlation id recorded by one of
/// `listener_machine_id`'s proxy entities, deliver it to the original sender
/// with the proxy's identity as the response sender, drop the correlation
/// record, and return `true`. Returns `false` when the packet is not a
/// proxied response for this machine.
pub fn try_route_proxy_response(
    app: &Arc<dyn ICore>,
    listener_machine_id: &str,
    value: &Value,
) -> bool {
    let correlation_id = extract_correlation_id(value);
    if correlation_id.is_empty() {
        return false;
    }
    let corr_key = correlation_key(&correlation_id);
    let corr_key_owned = corr_key.clone();
    let record = read_state(app, Map::new(), move |trx| {
        trx.get_json(&corr_key_owned, "record").unwrap_or_default()
    });
    if record.is_empty() {
        return false;
    }
    let rec = |k: &str| record.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    // Only the proxy that created the correlation may route the response —
    // the same correlation id travelling on the forwarded request must not
    // bounce at the target's own listener.
    if rec("proxyProgramId") != listener_machine_id {
        return false;
    }
    let sender_id = rec("senderId");
    if sender_id.is_empty() {
        return false;
    }
    let proxy_entity_id = rec("proxyEntityId");
    // Preserve the responder's payload; a non-Send-shaped packet (e.g. a raw
    // result object from a docker creature) is carried whole in `data`.
    let data = match value.get("data").and_then(Value::as_str) {
        Some(d) if !d.is_empty() => d.to_string(),
        _ => value.to_string(),
    };
    let response = StoresSend {
        user: proxy_identity(app, listener_machine_id),
        action: "single".to_string(),
        data,
        entity_id: proxy_entity_id,
        correlation_id: correlation_id.clone(),
        ..Default::default()
    };
    // The correlation is one round trip: consume it before delivery.
    app.modify_state(
        false,
        Box::new(move |trx: &dyn ITrx| {
            trx.del_json(&corr_key, "record");
            Ok(())
        }),
    );
    app.tools().signaler().signal_user(
        "creatures/signal",
        &sender_id,
        serde_json::to_value(&response).unwrap_or(Value::Null),
        true,
    );
    true
}

/// Request direction. If `entity_id` names a proxy entity of `machine_id`,
/// attach the entity's data file to the packet, record the correlation and
/// forward the repackaged signal to the configured target entity. Returns
/// `true` when the signal was consumed by a proxy entity.
pub fn try_forward_through_proxy(
    app: &Arc<dyn ICore>,
    machine_id: &str,
    entity_id: &str,
    value: &Value,
) -> bool {
    if entity_id.is_empty() {
        return false;
    }
    let machine_owned = machine_id.to_string();
    let entity_owned = entity_id.to_string();
    let (entity_type, data_path, config_raw) = read_state(
        app,
        (String::new(), String::new(), Map::new()),
        move |trx| {
            let etype =
                trx.get_link(&format!("vmEntityType::{}::{}", machine_owned, entity_owned));
            if etype != PROXY_RUNTIME_KEY {
                return (etype, String::new(), Map::new());
            }
            let path =
                trx.get_link(&format!("vmEntityPath::{}::{}", machine_owned, entity_owned));
            let cfg = trx
                .get_json(&config_key(&machine_owned, &entity_owned), "config")
                .unwrap_or_default();
            (etype, path, cfg)
        },
    );
    if entity_type != PROXY_RUNTIME_KEY {
        return false;
    }
    let config = ProxyConfig::from_value(&Value::Object(config_raw));
    if config.target_program_id.is_empty() {
        crate::drivers::vmm::bridge::runtime_io::log(format!(
            "proxy entity {}::{} has no target configured; dropping signal",
            machine_id, entity_id
        ));
        return true;
    }
    let send: StoresSend = serde_json::from_value(value.clone()).unwrap_or_default();
    // Reuse the sender's correlation id when provided so the requester can
    // match the response; otherwise mint one for the round trip.
    let correlation_id = if !send.correlation_id.is_empty() {
        send.correlation_id.clone()
    } else {
        let embedded = extract_correlation_id(value);
        if embedded.is_empty() {
            Uuid::new_v4().to_string()
        } else {
            embedded
        }
    };
    let attachment = if data_path.is_empty() {
        String::new()
    } else {
        fs::read_to_string(&data_path).unwrap_or_default()
    };
    // Attach the proxy's data file and routing envelope to the payload.
    let mut payload: Map<String, Value> = match serde_json::from_str::<Value>(&send.data) {
        Ok(Value::Object(o)) => o,
        _ => {
            let mut m = Map::new();
            if !send.data.is_empty() {
                m.insert("data".to_string(), json!(send.data));
            }
            m
        }
    };
    payload.insert(config.attach_field.clone(), json!(attachment));
    payload.insert("correlationId".to_string(), json!(correlation_id));
    payload.insert("replyTo".to_string(), json!(machine_id));
    payload.insert("proxyProgramId".to_string(), json!(machine_id));
    payload.insert("proxyEntityId".to_string(), json!(entity_id));
    let record = json!({
        "senderId": send.user.id,
        "proxyProgramId": machine_id,
        "proxyEntityId": entity_id,
        "targetProgramId": config.target_program_id,
        "targetEntityId": config.target_entity_id,
        "createdAt": chrono::Utc::now().timestamp_millis(),
    });
    let corr_key = correlation_key(&correlation_id);
    app.modify_state(
        false,
        Box::new(move |trx: &dyn ITrx| {
            let _ = trx.put_json(&corr_key, "record", &record, true);
            Ok(())
        }),
    );
    let forwarded = StoresSend {
        user: proxy_identity(app, machine_id),
        action: "single".to_string(),
        data: Value::Object(payload).to_string(),
        is_temp: send.is_temp,
        entity_id: config.target_entity_id.clone(),
        correlation_id,
        ..Default::default()
    };
    app.tools().signaler().signal_user(
        "creatures/signal",
        &config.target_program_id,
        serde_json::to_value(&forwarded).unwrap_or(Value::Null),
        true,
    );
    true
}
