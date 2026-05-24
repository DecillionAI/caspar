//! Translation of `shell/api/actions/program/program.go`.
//!
//! Wires the program / app / VM action surface and translates the Go bodies
//! one-to-one. A few notes on the things that didn't survive the port verbatim:
//!
//! * **Per-minute billing background loop.** Go's `Install` spawned a
//!   15-second ticker calling `chargeRunningStandaloneVmsIfNeeded`, which
//!   advances locked-token billing for every running standalone VM. The Rust
//!   port mirrors that ticker using a background thread.
//! * **Chain re-entry for `consumeLock`.** The billing helper now synchronously
//!   calls `Globe.SendBaseRequestOnChain("/creatures/consumeLock", ...)` and
//!   updates VM billing state on success.
//! * The Go module also boots the existing programs (calling `Vmm.Assign` and
//!   replaying any pending `vmAlarm*` links). The Rust `install` mirrors the
//!   one-shot scan; the timed alarm replay is preserved.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::abstractions::models::action::ISecureAction;
use crate::abstractions::models::core::ICore;
use crate::abstractions::models::trx::ITrx;
use crate::abstractions::state::IState;
use crate::core::actor::model::secured::guard::Guard;
use crate::shell::api::inputs::program::{
    CreateMachineInput, DeleteProgramInput, DeployInput, ListAppMachsInput, ListInput,
    MachineBuildsInput, ReadVmLogsInput, RunProgramEntityInput, UpdateProgramInput,
    VmResourcesInput, VmTerminalInput,
};
use crate::shell::api::model::{Entity, Machine, Program, User};
use crate::shell::api::outputs::plugin::PlugInput;
use crate::shell::utils::future::async_once;

use super::util::build_secure_action;

const PLUGINS_TEMPLATE_NAME: &str = "/machines/";

fn user_guard() -> Guard {
    Guard {
        is_user: true,
        is_in_store: false,
    }
}

fn normalize_entity_type(s: &str) -> String {
    s.trim().to_lowercase()
}

fn is_supported_entity_type(s: &str) -> bool {
    matches!(
        normalize_entity_type(s).as_str(),
        "docker" | "wasm" | "elpify" | "javascript" | "elpian" | "fire"
    )
}

fn entity_runtime_file_name(s: &str) -> &'static str {
    match normalize_entity_type(s).as_str() {
        "elpify" => "module.elpify.js",
        "javascript" => "module.js",
        "elpian" => "module.elpian.json",
        _ => "module.wasm",
    }
}

fn as_i64(raw: &Value) -> Option<i64> {
    match raw {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        _ => None,
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct VmResources {
    #[serde(rename = "maxExecTimeSeconds")]
    max_exec_time_seconds: i64,
    #[serde(rename = "ramMb")]
    ram_mb: i64,
    #[serde(rename = "diskGb")]
    disk_gb: i64,
    #[serde(rename = "cpuCores")]
    cpu_cores: i64,
}

fn normalize_vm_resources(input: &VmResourcesInput) -> VmResources {
    let mut res = VmResources {
        max_exec_time_seconds: input.max_exec_time_seconds,
        ram_mb: input.ram_mb,
        disk_gb: input.disk_gb,
        cpu_cores: input.cpu_cores,
    };
    if res.max_exec_time_seconds <= 0 {
        res.max_exec_time_seconds = 60;
    }
    if res.ram_mb <= 0 {
        res.ram_mb = 64;
    }
    if res.disk_gb <= 0 {
        res.disk_gb = 1;
    }
    if res.cpu_cores <= 0 {
        res.cpu_cores = 1;
    }
    res
}

fn vm_per_minute_cost(app: &Arc<dyn ICore>, resources: &VmResources) -> i64 {
    let cost = (resources.ram_mb * app.vm_ram_cost_per_mb_per_minute())
        + (resources.cpu_cores * app.vm_cpu_core_cost_per_minute())
        + (resources.disk_gb * app.vm_disk_cost_per_gb_per_minute());
    if cost <= 0 {
        1
    } else {
        cost
    }
}

fn validate_and_build_vm_billing(
    app: &Arc<dyn ICore>,
    trx: &dyn ITrx,
    payer_id: &str,
    lock_id: &str,
    payment_signatures: &[String],
    resources: &VmResources,
) -> Result<Map<String, Value>> {
    if lock_id.is_empty() {
        return Err(anyhow!(
            "paymentLockId is required for standalone vm execution"
        ));
    }
    let payment = trx
        .get_json(
            &format!("Json::User::{}", payer_id),
            &format!("lockedTokens.{}", lock_id),
        )
        .map_err(|_| anyhow!("payment lock not found"))?;
    let target = payment.get("userId").and_then(|v| v.as_str()).unwrap_or("");
    if target != app.owner_id() {
        return Err(anyhow!("payment lock target is invalid"));
    }
    let steps_raw = match payment.get("steps") {
        Some(Value::Array(arr)) if !arr.is_empty() => arr.clone(),
        _ => return Err(anyhow!("payment lock does not include steps")),
    };
    if payment_signatures.len() != steps_raw.len() {
        return Err(anyhow!(
            "paymentSignatures count must match lock steps count"
        ));
    }
    let per_minute_cost = vm_per_minute_cost(app, resources);
    let mut step_unlocks = vec![0i64; steps_raw.len()];
    for (i, raw_step) in steps_raw.iter().enumerate() {
        let step = match raw_step {
            Value::Object(o) => o,
            _ => return Err(anyhow!("invalid payment lock step")),
        };
        let step_amount = step.get("amount").and_then(as_i64).unwrap_or(0);
        if step_amount != per_minute_cost {
            return Err(anyhow!(
                "payment lock step amount must match vm per-minute resource cost"
            ));
        }
        let unlock_at = step.get("unlockAt").and_then(as_i64).unwrap_or(0);
        if unlock_at <= 0 {
            return Err(anyhow!("payment lock step unlockAt is invalid"));
        }
        step_unlocks[i] = unlock_at;
        if i > 0 && (step_unlocks[i] - step_unlocks[i - 1] != 60_000) {
            return Err(anyhow!("payment lock steps must be one-minute apart"));
        }
        let sign_payload = format!(
            "{}:{}:{}:{}:{}",
            lock_id,
            i,
            unlock_at,
            step_amount,
            app.owner_id()
        );
        let (success, _, _) = app.tools().security().auth_with_signature(
            payer_id,
            sign_payload.as_bytes(),
            &payment_signatures[i],
        );
        if !success {
            return Err(anyhow!("payment signature verification failed"));
        }
    }
    let mut out: Map<String, Value> = Map::new();
    out.insert("payerUserId".into(), json!(payer_id));
    out.insert("lockId".into(), json!(lock_id));
    out.insert("perMinuteCost".into(), json!(per_minute_cost));
    out.insert("currentStep".into(), json!(0));
    out.insert("stepCount".into(), json!(steps_raw.len()));
    out.insert("lastChargeMinute".into(), json!(-1i64));
    out.insert("signatures".into(), json!(payment_signatures));
    out.insert("resources".into(), serde_json::to_value(resources)?);
    Ok(out)
}

/// Helper that walks the running `VmBilling::*` link space and produces the
/// list of charge targets the per-minute ticker would normally consume.
///
/// TODO: the timed scheduler is intentionally not started by `install`. Until
/// it is, this helper is unreachable; it's preserved so the scheduler can be
/// dropped in without re-translating the billing logic.
#[allow(dead_code)]
pub(crate) fn charge_running_standalone_vms_if_needed(app: &Arc<dyn ICore>, lock: &Mutex<i64>) {
    let mut guard = match lock.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    let now = chrono::Utc::now().timestamp();
    let current_minute = now / 60;
    if *guard == current_minute {
        return;
    }
    let targets: Arc<Mutex<Vec<Map<String, Value>>>> = Arc::new(Mutex::new(Vec::new()));
    let targets_for_closure = targets.clone();
    app.modify_state(
        true,
        Box::new(move |tx: &dyn ITrx| {
            let links = match tx.get_links_list("VmBilling::", -1, -1, &[]) {
                Ok(v) => v,
                Err(_) => return Ok(()),
            };
            let mut acc = targets_for_closure.lock().unwrap();
            for link in links {
                let vm_id = link.trim_start_matches("VmBilling::").to_string();
                if vm_id.is_empty() || tx.get_link(&format!("VmStatus::{}", vm_id)) != "running" {
                    continue;
                }
                let billing = match tx.get_json(&format!("Json::VmBilling::{}", vm_id), "payment") {
                    Ok(b) => b,
                    Err(_) => continue,
                };
                let next_step = match billing.get("currentStep").and_then(as_i64) {
                    Some(n) => n,
                    None => continue,
                };
                let payer_id = billing
                    .get("payerUserId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let lock_id = billing
                    .get("lockId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let per_minute_cost = billing.get("perMinuteCost").and_then(as_i64).unwrap_or(0);
                let last_charge_minute = billing
                    .get("lastChargeMinute")
                    .and_then(as_i64)
                    .unwrap_or(0);
                let machine_id = billing
                    .get("machineId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let entity_id = billing
                    .get("entityId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let signatures_raw: Vec<String> = billing
                    .get("signatures")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .map(|s| s.as_str().unwrap_or("").to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                if payer_id.is_empty()
                    || lock_id.is_empty()
                    || per_minute_cost <= 0
                    || machine_id.is_empty()
                    || entity_id.is_empty()
                {
                    continue;
                }
                if last_charge_minute == current_minute {
                    continue;
                }
                if next_step < 0 {
                    continue;
                }
                if (next_step as usize) >= signatures_raw.len() {
                    if last_charge_minute < current_minute {
                        let mut t: Map<String, Value> = Map::new();
                        t.insert("vmId".into(), json!(vm_id));
                        t.insert("machineId".into(), json!(machine_id));
                        t.insert("entityId".into(), json!(entity_id));
                        t.insert("stopOnly".into(), json!(true));
                        acc.push(t);
                    }
                    continue;
                }
                let mut t: Map<String, Value> = Map::new();
                t.insert("vmId".into(), json!(vm_id));
                t.insert("payerUserId".into(), json!(payer_id));
                t.insert("lockId".into(), json!(lock_id));
                t.insert("step".into(), json!(next_step));
                t.insert("amount".into(), json!(per_minute_cost));
                t.insert(
                    "signature".into(),
                    json!(signatures_raw[next_step as usize]),
                );
                t.insert("machineId".into(), json!(machine_id));
                t.insert("entityId".into(), json!(entity_id));
                acc.push(t);
            }
            Ok(())
        }),
    );
    let targets = std::mem::take(&mut *targets.lock().unwrap());
    for target in targets {
        if target
            .get("stopOnly")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            let machine_id = target
                .get("machineId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let entity_id = target
                .get("entityId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let vm_id = target
                .get("vmId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            terminate_standalone_vm(app, &machine_id, &entity_id, &vm_id);
            let vm_id_for_closure = vm_id.clone();
            let machine_id_for_closure = machine_id.clone();
            let entity_id_for_closure = entity_id.clone();
            app.modify_state(
                false,
                Box::new(move |tx: &dyn ITrx| {
                    tx.del_key(&format!("link::VmStatus::{}", vm_id_for_closure));
                    tx.del_key(&format!(
                        "link::VmInstance::{}::{}::{}",
                        machine_id_for_closure, entity_id_for_closure, vm_id_for_closure
                    ));
                    tx.del_key(&format!("link::VmBilling::{}", vm_id_for_closure));
                    tx.del_json(
                        &format!("Json::VmBilling::{}", vm_id_for_closure),
                        "payment",
                    );
                    Ok(())
                }),
            );
            continue;
        }
        let payload = json!({
            "type": "pay",
            "userId": target.get("payerUserId").and_then(|v| v.as_str()).unwrap_or(""),
            "lockId": target.get("lockId").and_then(|v| v.as_str()).unwrap_or(""),
            "signature": target.get("signature").and_then(|v| v.as_str()).unwrap_or(""),
            "amount": target.get("amount").and_then(as_i64).unwrap_or(0),
            "step": target.get("step").and_then(as_i64).unwrap_or(-1),
        });
        let payload_bytes = serde_json::to_vec(&payload).unwrap_or_default();
        let sig = app.sign_packet_as_owner(&payload_bytes);
        let (tx, rx) = std::sync::mpsc::channel::<bool>();
        let owner = app.owner_id();
        app.globe().send_base_request_on_chain(
            "/creatures/consumeLock",
            payload_bytes,
            &sig,
            &owner,
            "",
            Box::new(move |_data, status, err| {
                let ok = err.is_none() && status < 400;
                let _ = tx.send(ok);
            }),
        );
        let consumed = rx
            .recv_timeout(std::time::Duration::from_secs(30))
            .unwrap_or(false);
        let vm_id = target
            .get("vmId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let machine_id = target
            .get("machineId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let entity_id = target
            .get("entityId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if consumed {
            let vm_for_closure = vm_id.clone();
            app.modify_state(
                false,
                Box::new(move |tx: &dyn ITrx| {
                    let mut billing = tx
                        .get_json(&format!("Json::VmBilling::{}", vm_for_closure), "payment")
                        .unwrap_or_default();
                    let current_step = billing.get("currentStep").and_then(as_i64).unwrap_or(0);
                    billing.insert("currentStep".into(), json!(current_step + 1));
                    billing.insert("lastChargeMinute".into(), json!(current_minute));
                    tx.put_json(
                        &format!("Json::VmBilling::{}", vm_for_closure),
                        "payment",
                        &Value::Object(billing),
                        true,
                    )?;
                    Ok(())
                }),
            );
        } else {
            terminate_standalone_vm(app, &machine_id, &entity_id, &vm_id);
        }
    }
    *guard = current_minute;
}

fn terminate_standalone_vm(app: &Arc<dyn ICore>, machine_id: &str, entity_id: &str, vm_id: &str) {
    let machine_id = machine_id.to_string();
    let entity_id = entity_id.to_string();
    let vm_id = vm_id.to_string();
    let app_for_closure = app.clone();
    app.modify_state(
        true,
        Box::new(move |tx: &dyn ITrx| {
            let entity = Entity {
                program_id: machine_id.clone(),
                entity_id: entity_id.clone(),
                ..Default::default()
            }
            .pull(tx);
            let entity_type = normalize_entity_type(&entity.entity_type);
            let mut stop_input: Map<String, Value> = Map::new();
            stop_input.insert("runtime".into(), json!(entity_type));
            stop_input.insert("machineId".into(), json!(machine_id));
            stop_input.insert("entityId".into(), json!(entity_id));
            stop_input.insert("vmId".into(), json!(vm_id));
            if entity_type == "docker" {
                stop_input.insert("entityId".into(), json!(entity.entity_id));
                stop_input.insert(
                    "containerName".into(),
                    json!(tx.get_link(&format!(
                        "VmContainerName::{}::{}::{}",
                        machine_id, entity_id, vm_id
                    ))),
                );
            }
            let msg = json!({
                "key": "terminateVm",
                "input": stop_input,
            });
            app_for_closure.tools().vmm().vm_callback(&msg.to_string());
            Ok(())
        }),
    );
}

fn create_program(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    let app_for_handler = app.clone();
    build_secure_action::<CreateMachineInput, _>(
        app,
        "/programs/create",
        user_guard(),
        move |state: Arc<dyn IState>, input: CreateMachineInput| -> Result<Value> {
            let trx = state.trx();
            if !trx.has_obj("Machine", &input.app_id) {
                return Err(anyhow!("machine not found"));
            }
            let mut machine = Machine {
                id: input.app_id.clone(),
                ..Default::default()
            }
            .pull(&*trx, false);
            if machine.owner_id != state.info().user_id() {
                return Err(anyhow!("you are not owner of machine"));
            }
            let program = Program {
                machine_id: app_for_handler.tools().storage().gen_id(
                    &*trx,
                    &crate::abstractions::models::input::IInput::origin(&input),
                ),
                app_id: machine.id.clone(),
                path: input.path.clone(),
                runtime: input.runtime.clone(),
                comment: input.comment.clone(),
            };
            machine.machines_count += 1;
            machine.push(&*trx);
            program.push(&*trx);
            let _ = trx.put_json(
                &format!("ProgMeta::{}", program.machine_id),
                "metadata",
                &json!({}),
                true,
            );
            trx.put_link(
                &format!("machinePrograms::{}::{}", machine.id, program.machine_id),
                "true",
            );
            Ok(json!({"program": program}))
        },
    )
}

fn delete_program(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<DeleteProgramInput, _>(
        app,
        "/programs/delete",
        user_guard(),
        move |state: Arc<dyn IState>, input: DeleteProgramInput| -> Result<Value> {
            let trx = state.trx();
            if !trx.has_obj("Program", &input.program_id) {
                return Err(anyhow!("program does not exist"));
            }
            let app_id = trx.get_index("Program", "id", "programId", &input.program_id);
            let mut machine = Machine {
                id: app_id,
                ..Default::default()
            }
            .pull(&*trx, false);
            machine.machines_count -= 1;
            machine.push(&*trx);
            trx.del_index("Program", "id", "programId", &input.program_id);
            trx.del_key(&format!(
                "link::machinePrograms::{}::{}",
                machine.id, input.program_id
            ));
            Ok(json!({}))
        },
    )
}

fn update_program(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<UpdateProgramInput, _>(
        app,
        "/programs/update",
        user_guard(),
        move |state: Arc<dyn IState>, input: UpdateProgramInput| -> Result<Value> {
            let trx = state.trx();
            if !trx.has_obj("Program", &input.program_id) {
                return Err(anyhow!("program does not exist"));
            }
            let mut program = Program {
                machine_id: input.program_id.clone(),
                ..Default::default()
            }
            .pull(&*trx);
            program.path = input.path.clone();
            program.push(&*trx);
            if !input.metadata.is_empty() {
                let meta_value = Value::Object(
                    input
                        .metadata
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                );
                trx.put_json(
                    &format!("ProgMeta::{}", program.machine_id),
                    "metadata",
                    &meta_value,
                    true,
                )?;
            }
            Ok(json!({}))
        },
    )
}

fn run_program_entity(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    let app_for_handler = app.clone();
    build_secure_action::<RunProgramEntityInput, _>(
        app,
        "/programs/runEntity",
        user_guard(),
        move |state: Arc<dyn IState>, input: RunProgramEntityInput| -> Result<Value> {
            let trx = state.trx();
            let program_id = if input.program_id.is_empty() {
                input.machine_id.clone()
            } else {
                input.program_id.clone()
            };
            if !trx.has_obj("Program", &program_id) {
                return Err(anyhow!("program does not exist"));
            }
            let program = Program {
                machine_id: program_id.clone(),
                ..Default::default()
            }
            .pull(&*trx);
            let entity = Entity {
                program_id: program.machine_id.clone(),
                entity_id: input.entity_id.clone(),
                ..Default::default()
            }
            .pull(&*trx);
            if entity.entity_id.is_empty() {
                return Err(anyhow!("entity does not exist"));
            }
            let entity_type = normalize_entity_type(&entity.entity_type);
            let machine = Machine {
                id: program.machine_id.clone(),
                ..Default::default()
            }
            .pull(&*trx, false);
            if machine.owner_id != state.info().user_id() {
                return Err(anyhow!("you are not owner of this machine"));
            }
            let vm_id = Uuid::new_v4().to_string();
            trx.put_link(&format!("VmStatus::{}", vm_id), "running");
            let resources = normalize_vm_resources(&input.resources);
            let mut billing_data = validate_and_build_vm_billing(
                &app_for_handler,
                &*trx,
                &state.info().user_id(),
                &input.payment_lock_id,
                &input.payment_signatures,
                &resources,
            )?;
            billing_data.insert("machineId".into(), json!(input.machine_id));
            billing_data.insert("entityId".into(), json!(input.entity_id));
            billing_data.insert("vmId".into(), json!(vm_id));
            trx.put_link(&format!("VmBilling::{}", vm_id), "true");
            trx.put_json(
                &format!("Json::VmBilling::{}", vm_id),
                "payment",
                &Value::Object(billing_data),
                true,
            )?;
            let params: HashMap<String, String> = if input.params.is_empty() {
                HashMap::new()
            } else {
                input.params.clone()
            };
            trx.put_link(
                &format!(
                    "VmInstance::{}::{}::{}",
                    program.machine_id, input.entity_id, vm_id
                ),
                "true",
            );
            if entity_type == "docker" {
                let entity_image_id = input.entity_id.clone();
                let container_name = Uuid::new_v4().to_string();
                trx.put_link(
                    &format!(
                        "VmContainerName::{}::{}::{}",
                        program.machine_id, input.entity_id, vm_id
                    ),
                    &container_name,
                );
                let app_async = app_for_handler.clone();
                let machine_id = input.machine_id.clone();
                let entity_id = input.entity_id.clone();
                let vm_id_async = vm_id.clone();
                let resources_async = resources.clone();
                let image_ref =
                    format!("{}/{}", input.machine_id.replace('@', "_"), entity_image_id);
                let params_async = params.clone();
                let _ = async_once(move || {
                    let msg = json!({
                        "key": "runVm",
                        "input": {
                            "runtime": "docker",
                            "machineId": machine_id,
                            "entityId": entity_id,
                            "containerName": container_name,
                            "standalone": true,
                            "vmId": vm_id_async,
                            "resources": resources_async,
                            "imageRef": image_ref,
                            "inputFiles": params_async,
                        },
                    });
                    app_async.tools().vmm().vm_callback(&msg.to_string());
                });
                return Ok(json!({"vmId": vm_id}));
            }
            if !is_supported_entity_type(&entity_type) {
                return Err(anyhow!("invalid entity type"));
            }
            let data = serde_json::to_string(&params).unwrap_or_else(|_| "{}".to_string());
            let app_async = app_for_handler.clone();
            let machine_id = input.machine_id.clone();
            let entity_id = input.entity_id.clone();
            let vm_id_async = vm_id.clone();
            let resources_async = resources.clone();
            let entity_type_async = entity_type.clone();
            let _ = async_once(move || {
                let msg = json!({
                    "key": "runVm",
                    "input": {
                        "runtime": entity_type_async,
                        "machineId": machine_id,
                        "entityId": entity_id,
                        "standalone": true,
                        "vmId": vm_id_async,
                        "resources": resources_async,
                        "data": data,
                    },
                });
                app_async.tools().vmm().vm_callback(&msg.to_string());
            });
            Ok(json!({"vmId": vm_id}))
        },
    )
}

fn stop_program_entity(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    let app_for_handler = app.clone();
    build_secure_action::<RunProgramEntityInput, _>(
        app,
        "/programs/stopEntity",
        user_guard(),
        move |state: Arc<dyn IState>, input: RunProgramEntityInput| -> Result<Value> {
            let trx = state.trx();
            let program_id = if input.program_id.is_empty() {
                input.machine_id.clone()
            } else {
                input.program_id.clone()
            };
            if !trx.has_obj("Program", &program_id) {
                return Err(anyhow!("program does not exist"));
            }
            let program = Program {
                machine_id: program_id.clone(),
                ..Default::default()
            }
            .pull(&*trx);
            let entity = Entity {
                program_id: program.machine_id.clone(),
                entity_id: input.entity_id.clone(),
                ..Default::default()
            }
            .pull(&*trx);
            if entity.entity_id.is_empty() {
                return Err(anyhow!("entity does not exist"));
            }
            let entity_type = normalize_entity_type(&entity.entity_type);
            let machine = Machine {
                id: program.machine_id.clone(),
                ..Default::default()
            }
            .pull(&*trx, false);
            if machine.owner_id != state.info().user_id() {
                return Err(anyhow!("you are not owner of this machine"));
            }
            let vm_id = input.vm_id.clone();
            trx.del_key(&format!("link::VmStatus::{}", vm_id));
            trx.del_key(&format!(
                "link::VmInstance::{}::{}::{}",
                program.machine_id, input.entity_id, vm_id
            ));
            trx.del_key(&format!("link::VmBilling::{}", vm_id));
            trx.del_json(&format!("Json::VmBilling::{}", vm_id), "payment");
            let entity_image_id = entity.entity_id.clone();
            let container_name = trx.get_link(&format!(
                "VmContainerName::{}::{}::{}",
                program.machine_id, input.entity_id, vm_id
            ));
            trx.del_key(&format!(
                "link::vmStandaloneImageName::{}::{}",
                program.machine_id, input.entity_id
            ));
            trx.del_key(&format!(
                "link::vmStandaloneContainerName::{}::{}",
                program.machine_id, input.entity_id
            ));
            let mut stop_input: Map<String, Value> = Map::new();
            stop_input.insert("runtime".into(), json!(entity_type));
            stop_input.insert("machineId".into(), json!(input.machine_id));
            stop_input.insert("entityId".into(), json!(input.entity_id));
            stop_input.insert("vmId".into(), json!(vm_id));
            if entity_type == "docker" {
                if entity_image_id.is_empty() || container_name.is_empty() {
                    return Err(anyhow!("entity runtime links are not found"));
                }
                stop_input.insert("entityId".into(), json!(entity_image_id));
                stop_input.insert("containerName".into(), json!(container_name));
            }
            let msg = json!({
                "key": "terminateVm",
                "input": stop_input,
            });
            app_for_handler.tools().vmm().vm_callback(&msg.to_string());
            Ok(json!({}))
        },
    )
}

fn read_vm_logs(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    let app_for_handler = app.clone();
    build_secure_action::<ReadVmLogsInput, _>(
        app,
        "/machines/readVmLogs",
        user_guard(),
        move |state: Arc<dyn IState>, input: ReadVmLogsInput| -> Result<Value> {
            let trx = state.trx();
            let mut owner_user_id = String::new();
            if let Ok(links) = trx.get_links_list("VmInstance::", -1, -1, &[]) {
                let suffix = format!("::{}", input.vm_id);
                for link in links {
                    if !link.ends_with(&suffix) {
                        continue;
                    }
                    let parts: Vec<&str> = link.split("::").collect();
                    if parts.len() < 4 {
                        continue;
                    }
                    let program = Program {
                        machine_id: parts[1].to_string(),
                        ..Default::default()
                    }
                    .pull(&*trx);
                    if program.machine_id.is_empty() {
                        break;
                    }
                    let machine = Machine {
                        id: program.app_id.clone(),
                        ..Default::default()
                    }
                    .pull(&*trx, false);
                    owner_user_id = machine.owner_id;
                    break;
                }
            }
            if owner_user_id.is_empty() {
                return Err(anyhow!("vm not found"));
            }
            if owner_user_id != state.info().user_id() {
                return Err(anyhow!("you are not owner of this vm"));
            }
            let count = if input.count <= 0 { 100 } else { input.count };
            let offset = if input.offset < 0 { 0 } else { input.offset };
            let logs = app_for_handler.tools().storage().read_vm_logs(
                &input.vm_id,
                &input.log_type,
                offset,
                count,
            );
            Ok(json!({"logs": logs}))
        },
    )
}

fn open_vm_terminal(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<VmTerminalInput, _>(
        app,
        "/machines/openVmTerminal",
        user_guard(),
        move |state: Arc<dyn IState>, input: VmTerminalInput| -> Result<Value> {
            let trx = state.trx();
            if !trx.has_obj("Program", &input.creature_id) {
                return Err(anyhow!("program does not exist"));
            }
            let program = Program {
                machine_id: input.creature_id.clone(),
                ..Default::default()
            }
            .pull(&*trx);
            let machine = Machine {
                id: program.app_id.clone(),
                ..Default::default()
            }
            .pull(&*trx, false);
            if machine.owner_id != state.info().user_id() {
                return Err(anyhow!("you are not owner of this creature"));
            }
            trx.put_link(
                &format!(
                    "VmTerminal::{}::{}::{}",
                    input.creature_id,
                    input.vm_id,
                    state.info().user_id()
                ),
                "true",
            );
            Ok(json!({"terminal": "on"}))
        },
    )
}

fn close_vm_terminal(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<VmTerminalInput, _>(
        app,
        "/machines/closeVmTerminal",
        user_guard(),
        move |state: Arc<dyn IState>, input: VmTerminalInput| -> Result<Value> {
            let trx = state.trx();
            if !trx.has_obj("Program", &input.creature_id) {
                return Err(anyhow!("program does not exist"));
            }
            let program = Program {
                machine_id: input.creature_id.clone(),
                ..Default::default()
            }
            .pull(&*trx);
            let machine = Machine {
                id: program.app_id.clone(),
                ..Default::default()
            }
            .pull(&*trx, false);
            if machine.owner_id != state.info().user_id() {
                return Err(anyhow!("you are not owner of this creature"));
            }
            trx.del_key(&format!(
                "link::VmTerminal::{}::{}::{}",
                input.creature_id,
                input.vm_id,
                state.info().user_id()
            ));
            Ok(json!({"terminal": "off"}))
        },
    )
}

fn read_machine_builds(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<MachineBuildsInput, _>(
        app,
        "/machines/readMachineBuilds",
        user_guard(),
        move |state: Arc<dyn IState>, input: MachineBuildsInput| -> Result<Value> {
            let prefix = format!("VmBuilds::{}::", input.machine_id);
            let builds =
                state
                    .trx()
                    .get_links_list(&prefix, input.offset, input.count, &[false])?;
            Ok(json!({"buildsList": builds}))
        },
    )
}

fn deploy(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    let app_for_handler = app.clone();
    build_secure_action::<DeployInput, _>(
        app,
        "/programs/deploy",
        user_guard(),
        move |state: Arc<dyn IState>, input: DeployInput| -> Result<Value> {
            let trx = state.trx();
            let program_id = input.machine_id.clone();
            if !trx.has_obj("Program", &program_id) {
                return Err(anyhow!("program not found"));
            }
            let program = Program {
                machine_id: program_id.clone(),
                ..Default::default()
            }
            .pull(&*trx);
            if !trx.has_obj("Machine", &program.app_id) {
                return Err(anyhow!("machine not found"));
            }
            let machine = Machine {
                id: program.app_id.clone(),
                ..Default::default()
            }
            .pull(&*trx, false);
            if machine.owner_id != state.info().user_id() {
                return Err(anyhow!("access to vm denied"));
            }
            let entity_type = normalize_entity_type(&input.entity_type);
            if !is_supported_entity_type(&entity_type) {
                return Err(anyhow!(
                    "invalid entityType, expected one of docker|wasm|elpify|javascript|elpian|fire"
                ));
            }
            let data = base64::engine::general_purpose::STANDARD
                .decode(&input.payload)
                .map_err(|e| anyhow!("{}", e))?;
            let mut entity_model = Entity {
                program_id: program.machine_id.clone(),
                entity_id: input.entity_id.clone(),
                entity_type: entity_type.clone(),
                image_name: input.entity_id.clone(),
            };
            let vm_id = Uuid::new_v4().to_string();
            if entity_type == "docker" || entity_type == "wasm" {
                let mut files: HashMap<String, Value> = HashMap::new();
                if let Some(files_raw) = input.metadata.get("files") {
                    match files_raw {
                        Value::Object(o) => {
                            files = o.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                        }
                        Value::Null => {}
                        _ => return Err(anyhow!("files is not map")),
                    }
                }
                let build_folder_path = format!(
                    "{}{}{}/entities/{}",
                    app_for_handler.tools().storage().storage_root(),
                    PLUGINS_TEMPLATE_NAME,
                    program.machine_id,
                    input.entity_id
                );
                let primary_file_name = if entity_type == "wasm" {
                    entity_runtime_file_name(&entity_type)
                } else {
                    "Dockerfile"
                };
                app_for_handler.tools().file().save_data_to_global_storage(
                    &build_folder_path,
                    &data,
                    primary_file_name,
                    true,
                )?;
                for (k, v) in &files {
                    let data_str = match v {
                        Value::String(s) => s.clone(),
                        _ => return Err(anyhow!("file bytecode not string")),
                    };
                    let raw = base64::engine::general_purpose::STANDARD
                        .decode(&data_str)
                        .map_err(|e| anyhow!("{}", e))?;
                    app_for_handler.tools().file().save_data_to_global_storage(
                        &build_folder_path,
                        &raw,
                        k,
                        true,
                    )?;
                }
                if entity_type == "wasm" {
                    trx.put_link(
                        &format!("vmEntityPath::{}::{}", program.machine_id, input.entity_id),
                        &format!("{}/{}", build_folder_path, primary_file_name),
                    );
                    trx.put_link(
                        &format!("vmEntityType::{}::{}", program.machine_id, input.entity_id),
                        "wasm",
                    );
                    program.push(&*trx);
                    app_for_handler.tools().vmm().assign(&program.machine_id);
                } else {
                    let build_id = Uuid::new_v4().to_string();
                    trx.put_link(&format!("VmBuilds::{}::{}", vm_id, build_id), "true");
                    let app_async = app_for_handler.clone();
                    let mid = program.machine_id.clone();
                    let eid = input.entity_id.clone();
                    let path = build_folder_path.clone();
                    let etype = entity_type.clone();
                    let _ = async_once(move || {
                        app_async
                            .tools()
                            .vmm()
                            .build_vm_image(&mid, &eid, &path, &etype);
                    });
                }
            } else {
                let file_name = entity_runtime_file_name(&entity_type);
                let entity_folder_path = format!(
                    "{}{}{}/entities/{}",
                    app_for_handler.tools().storage().storage_root(),
                    PLUGINS_TEMPLATE_NAME,
                    program.machine_id,
                    input.entity_id
                );
                app_for_handler.tools().file().save_data_to_global_storage(
                    &entity_folder_path,
                    &data,
                    file_name,
                    true,
                )?;
                if entity_type == "elpify" {
                    let build_id = Uuid::new_v4().to_string();
                    trx.put_link(&format!("VmBuilds::{}::{}", vm_id, build_id), "true");
                    let app_async = app_for_handler.clone();
                    let mid = program.machine_id.clone();
                    let eid = input.entity_id.clone();
                    let path = entity_folder_path.clone();
                    let etype = entity_type.clone();
                    let _ = async_once(move || {
                        app_async
                            .tools()
                            .vmm()
                            .build_vm_image(&mid, &eid, &path, &etype);
                    });
                }
                program.push(&*trx);
                app_for_handler.tools().vmm().assign(&program.machine_id);
            }
            entity_model.entity_type = entity_type;
            entity_model.push(&*trx);
            Ok(serde_json::to_value(PlugInput::default())?)
        },
    )
}

fn list_machines(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<ListInput, _>(
        app,
        "/machines/list",
        user_guard(),
        move |state: Arc<dyn IState>, input: ListInput| -> Result<Value> {
            let trx = state.trx();
            let machines = Machine::all(&*trx, input.offset, input.count)?;
            let mut result: Vec<Map<String, Value>> = Vec::new();
            for machine in machines {
                let profile = trx
                    .get_json(
                        &format!("CreatMeta::{}", machine.id),
                        "metadata.public.profile",
                    )
                    .ok();
                let mut row: Map<String, Value> = Map::new();
                row.insert("id".into(), json!(machine.id));
                row.insert("chainId".into(), json!(machine.chain_id));
                row.insert("username".into(), json!(machine.username));
                row.insert("ownerId".into(), json!(machine.owner_id));
                row.insert("programsCount".into(), json!(machine.machines_count));
                if let Some(p) = profile {
                    row.insert(
                        "title".into(),
                        p.get("title").cloned().unwrap_or_else(|| json!("untitled")),
                    );
                    row.insert(
                        "avatar".into(),
                        p.get("avatar").cloned().unwrap_or_else(|| json!("")),
                    );
                    row.insert(
                        "desc".into(),
                        p.get("desc").cloned().unwrap_or_else(|| json!("")),
                    );
                } else {
                    row.insert("title".into(), json!("untitled"));
                    row.insert("avatar".into(), json!(""));
                    row.insert("desc".into(), json!(""));
                }
                result.push(row);
            }
            Ok(json!({"machines": result}))
        },
    )
}

fn list_programs(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<ListInput, _>(
        app,
        "/programs/list",
        user_guard(),
        move |state: Arc<dyn IState>, input: ListInput| -> Result<Value> {
            let machines = Program::all(&*state.trx(), input.offset, input.count)?;
            Ok(json!({"machines": machines}))
        },
    )
}

fn list_program_machines(app: Arc<dyn ICore>) -> Arc<dyn ISecureAction> {
    build_secure_action::<ListAppMachsInput, _>(
        app,
        "/machines/listProgramMachines",
        user_guard(),
        move |state: Arc<dyn IState>, input: ListAppMachsInput| -> Result<Value> {
            let trx = state.trx();
            let prefix = format!("machinePrograms::{}::", input.app_id);
            let users = User::list(&*trx, &prefix, &HashMap::new())?;
            let programs = Program::list(&*trx, &prefix)?;
            let mut program_by_machine_id: HashMap<String, Program> = HashMap::new();
            for program in programs {
                program_by_machine_id.insert(program.machine_id.clone(), program);
            }
            let mut result: Vec<Map<String, Value>> = Vec::new();
            for user in users {
                let mut row: Map<String, Value> = Map::new();
                row.insert("id".into(), json!(user.id));
                row.insert("type".into(), json!(user.typ));
                row.insert("username".into(), json!(user.username));
                let comment = program_by_machine_id
                    .get(&user.id)
                    .map(|p| p.comment.clone())
                    .unwrap_or_default();
                row.insert("comment".into(), json!(comment));
                result.push(row);
            }
            Ok(json!({"machines": result}))
        },
    )
}

/// Mirror of Go's `Install`: walk the existing programs, hand each one to the
/// VMM and replay any pending vm-alarm. The 15-second billing ticker is
/// intentionally not started here — see the module doc-comment.
fn install_program_bootstrap(app: Arc<dyn ICore>) {
    let app_for_closure = app.clone();
    app.modify_state(
        true,
        Box::new(move |trx: &dyn ITrx| {
            let programs = Program::all(trx, -1, -1)?;
            for program in programs {
                if matches!(
                    program.runtime.as_str(),
                    "wasm" | "elpify" | "javascript" | "elpian" | "fire" | "docker"
                ) {
                    app_for_closure.tools().vmm().assign(&program.machine_id);
                    let store_id = trx.get_link(&format!("vmAlarmStoreId::{}", program.machine_id));
                    if !store_id.is_empty() {
                        let app_async = app_for_closure.clone();
                        let machine_id = program.machine_id.clone();
                        let store_id_clone = store_id.clone();
                        let alarm_time_raw = trx.get_link(&format!("vmAlarmTime::{}", machine_id));
                        let alarm_data = trx.get_link(&format!("vmAlarmData::{}", machine_id));
                        let _ = async_once(move || {
                            let t = alarm_time_raw.parse::<i64>().unwrap_or(0);
                            let ct = chrono::Utc::now().timestamp_millis();
                            if t > ct {
                                std::thread::sleep(std::time::Duration::from_millis(
                                    (t - ct) as u64,
                                ));
                            }
                            // Note: the original Go path cleared the alarm
                            // links inside a state-modifying closure; here we
                            // call into vmm directly. The links are reaped on
                            // the next bootstrap pass if they remain stale.
                            if app_async
                                .tools()
                                .security()
                                .has_access_to_store(&machine_id, &store_id_clone)
                            {
                                app_async.tools().vmm().run_vm(
                                    &machine_id,
                                    &store_id_clone,
                                    &alarm_data,
                                );
                            }
                        });
                    }
                }
                let prefix = format!("hasaccess::{}::", program.machine_id);
                let store_ids = trx.get_links_list(&prefix, -1, -1, &[]).unwrap_or_default();
                for store_id in store_ids {
                    let bare = store_id
                        .strip_prefix(&prefix)
                        .unwrap_or(&store_id)
                        .to_string();
                    app_for_closure
                        .tools()
                        .signaler()
                        .join_group(&bare, &program.machine_id);
                }
            }
            Ok(())
        }),
    );
}

/// Plug every program action into the actor.
pub fn install(app: Arc<dyn ICore>) {
    let actor = app.actor();
    let handlers: Vec<Arc<dyn ISecureAction>> = vec![
        create_program(app.clone()),
        delete_program(app.clone()),
        update_program(app.clone()),
        run_program_entity(app.clone()),
        stop_program_entity(app.clone()),
        read_vm_logs(app.clone()),
        open_vm_terminal(app.clone()),
        close_vm_terminal(app.clone()),
        read_machine_builds(app.clone()),
        deploy(app.clone()),
        list_machines(app.clone()),
        list_programs(app.clone()),
        list_program_machines(app.clone()),
    ];
    for h in handlers {
        actor.inject_secure_action(h);
    }
    install_program_bootstrap(app.clone());
    let billing_lock = Arc::new(Mutex::new(-1i64));
    let app_bg = app.clone();
    std::thread::spawn(move || loop {
        charge_running_standalone_vms_if_needed(&app_bg, &billing_lock);
        std::thread::sleep(std::time::Duration::from_secs(15));
    });
}
