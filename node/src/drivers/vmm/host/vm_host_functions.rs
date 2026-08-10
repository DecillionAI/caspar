use crate::drivers::vmm::prelude::*;
use crate::drivers::vmm::globals::with_global_app;
use crate::drivers::vmm::bridge::runtime_io::wasm_send;
use crate::drivers::vmm::host::functions::*;

pub(crate) struct HostHierarchy {
    pub(crate) vm_id: String,
    pub(crate) creature_id: String,
    pub(crate) program_id: String,
    pub(crate) entity_name: String,
    pub(crate) entity_path: String,
}

#[derive(Default)]
pub(crate) struct CachedVmHierarchy {
    pub(crate) creature_id: String,
    pub(crate) program_id: String,
}

fn resolve_cached_vm_hierarchy(packet: &JsonValue, input: &JsonValue) -> CachedVmHierarchy {
    // The vmId used to resolve the out-of-band VM context must be the
    // authoritative one the runtime stamped on the *packet* (the guest cannot
    // reach it). Only fall back to `input.vmId` for runtimes that stamp their
    // verified identity into the input envelope (e.g. the docker gateway),
    // never letting an in-process guest pick which VM context it resolves to.
    let vm_id = packet["vmId"]
        .as_str()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| input["vmId"].as_str().filter(|v| !v.trim().is_empty()))
        .unwrap_or("")
        .trim()
        .to_string();
    if vm_id.is_empty() {
        return CachedVmHierarchy::default();
    }
    with_global_app(|app| {
        app.tools().vmm().get_vm_context(&vm_id).map(|(creature_id, program_id)| {
            CachedVmHierarchy { creature_id, program_id }
        })
    })
    .flatten()
    .unwrap_or_default()
}

fn value_from_packet_or_input<'a>(
    packet: &'a JsonValue,
    input: &'a JsonValue,
    key: &str,
) -> &'a str {
    packet[key]
        .as_str()
        .filter(|v| !v.is_empty())
        .or_else(|| input[key].as_str().filter(|v| !v.is_empty()))
        .unwrap_or("")
}

pub(crate) fn resolve_host_hierarchy(packet: &JsonValue, input: &JsonValue) -> HostHierarchy {
    // Prefer the authoritative, runtime-stamped packet vmId over anything the
    // guest may have placed in `input`.
    let vm_id = packet["vmId"]
        .as_str()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| input["vmId"].as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let cached = resolve_cached_vm_hierarchy(packet, input);

    let creature_from_req = value_from_packet_or_input(packet, input, "creatureId");
    let creature_id_owned = if !cached.creature_id.is_empty() {
        cached.creature_id
    } else {
        creature_from_req.to_string()
    };

    let program_from_req = value_from_packet_or_input(packet, input, "programId");
    let program_id_owned = if !cached.program_id.is_empty() {
        cached.program_id
    } else {
        program_from_req.to_string()
    };

    let entity_name = value_from_packet_or_input(packet, input, "entityName").to_string();
    let entity_path = packet["entityPath"]
        .as_str()
        .filter(|v| !v.is_empty())
        .or_else(|| packet["astPath"].as_str().filter(|v| !v.is_empty()))
        .or_else(|| input["entityPath"].as_str().filter(|v| !v.is_empty()))
        .or_else(|| input["astPath"].as_str().filter(|v| !v.is_empty()))
        .or_else(|| input["astpath"].as_str().filter(|v| !v.is_empty()))
        .unwrap_or("")
        .to_string();
    HostHierarchy {
        vm_id,
        creature_id: creature_id_owned,
        program_id: program_id_owned,
        entity_name,
        entity_path,
    }
}

/// Execute a low-level key-value DB operation for a VM host-call.
///
/// All logic (write-ahead buffering, read-your-own-writes, ICore fallthrough)
/// lives in `IVmm::vm_db_op` on the `Vmm` struct, which is reached via the
/// canonical `ICore → tools() → vmm()` path.  This function is a thin
/// adapter that computes the storage namespace from `HostHierarchy` and
/// delegates.
pub(crate) fn run_db_op(ctx: &HostHierarchy, input: &JsonValue) -> Result<String, String> {
    let op  = input["op"].as_str().unwrap_or("");
    let key = input["key"].as_str().unwrap_or("");
    let val = input["val"].as_str().unwrap_or("");
    let prefix = input["prefix"].as_str().unwrap_or("");

    let db_prefix = if !ctx.creature_id.is_empty() && !ctx.program_id.is_empty() {
        if !ctx.entity_name.is_empty() && !ctx.entity_path.is_empty() {
            format!(
                "{}::{}::{}::{}",
                ctx.creature_id, ctx.program_id, ctx.entity_name, ctx.entity_path
            )
        } else if !ctx.entity_name.is_empty() {
            format!("{}::{}::{}", ctx.creature_id, ctx.program_id, ctx.entity_name)
        } else {
            format!("{}::{}", ctx.creature_id, ctx.program_id)
        }
    } else if !ctx.program_id.is_empty() {
        ctx.program_id.clone()
    } else {
        ctx.creature_id.clone()
    };

    let namespaced_key = format!("AppletDb::{}::{}", db_prefix, key);
    let ns_prefix = format!("AppletDb::{}::{}", db_prefix, prefix);

    match with_global_app(|app| {
        app.tools().vmm().vm_db_op(&ctx.vm_id, op, &namespaced_key, val, &ns_prefix)
    }) {
        Some(result) => result,
        None => Err("vmm not initialised".to_string()),
    }
}

pub(crate) fn perform_http_request(input: &JsonValue) -> Result<String, String> {
    let mut url = input["url"].as_str().unwrap_or("").to_string();
    if url.is_empty() {
        return Err("url is required".to_string());
    }

    let mut method = input["method"].as_str().unwrap_or("").trim().to_uppercase();
    if method.is_empty() {
        if let Some((prefixed_method, rest_url)) = url.split_once('|') {
            method = prefixed_method.trim().to_uppercase();
            url = rest_url.to_string();
        } else {
            method = "POST".to_string();
        }
    }

    let http_method =
        Method::from_bytes(method.as_bytes()).map_err(|e| format!("invalid http method: {}", e))?;

    let mut request = Client::new().request(http_method, url);

    match &input["headers"] {
        JsonValue::Object(headers_obj) => {
            for (k, v) in headers_obj {
                if let Some(value) = v.as_str() {
                    request = request.header(k, value);
                } else {
                    request = request.header(k, v.to_string());
                }
            }
        }
        JsonValue::String(headers_raw) => {
            if !headers_raw.trim().is_empty() {
                let parsed_headers: JsonValue = serde_json::from_str(headers_raw)
                    .map_err(|e| format!("invalid headers json: {}", e))?;
                if let Some(headers_obj) = parsed_headers.as_object() {
                    for (k, v) in headers_obj {
                        if let Some(value) = v.as_str() {
                            request = request.header(k, value);
                        } else {
                            request = request.header(k, v.to_string());
                        }
                    }
                } else {
                    return Err("headers must be a JSON object".to_string());
                }
            }
        }
        JsonValue::Null => {}
        _ => return Err("headers must be a JSON object or stringified JSON object".to_string()),
    }

    if let Some(body) = input["body"].as_str() {
        request = request.body(body.to_string());
    } else if !input["body"].is_null() {
        request = request.body(input["body"].to_string());
    }

    let response = request
        .send()
        .map_err(|e| format!("http request failed: {}", e))?;
    let bytes = response
        .bytes()
        .map_err(|e| format!("failed to read response body: {}", e))?;
    Ok(BASE64_STANDARD.encode(bytes))
}

pub(crate) fn handle_unified_host_call(packet: &JsonValue) -> String {
    let op = packet["op"]
        .as_str()
        .or_else(|| packet["key"].as_str())
        .unwrap_or("");
    let mut input = if packet["input"].is_null() {
        JsonValue::Null
    } else {
        packet["input"].clone()
    };
    let ctx = resolve_host_hierarchy(packet, &input);
    if ctx.program_id.is_empty() {
        return json!({"ok": false, "error": "programId is required"}).to_string();
    }
    if let Some(input_obj) = input.as_object_mut() {
        if input_obj
            .get("programId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .is_empty()
        {
            input_obj.insert(
                "programId".to_string(),
                JsonValue::String(ctx.program_id.clone()),
            );
        }
        if input_obj
            .get("machineId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .is_empty()
        {
            input_obj.insert(
                "machineId".to_string(),
                JsonValue::String(ctx.program_id.clone()),
            );
        }
    }
    match op {
        "commitTrx" => {
            let vm_id = input["vmId"]
                .as_str()
                .or_else(|| packet["vmId"].as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if vm_id.is_empty() {
                json!({"ok": false, "error": "vmId required for commitTrx"}).to_string()
            } else {
                match with_global_app(|app| app.tools().vmm().vm_db_commit_explicit(&vm_id)) {
                    Some(Ok(())) => json!({"ok": true}).to_string(),
                    Some(Err(e)) => json!({"ok": false, "error": e}).to_string(),
                    None         => json!({"ok": false, "error": "vmm not initialised"}).to_string(),
                }
            }
        }
        "dbOp" => host_fn_db_op(&ctx, &input),
        "runVm" => host_fn_run_vm(&input),
        "terminateVm" => host_fn_terminate_vm(&input),
        "execVm" | "execDocker" => host_fn_exec_vm(&input),
        "copyToVm" | "copyToDocker" => host_fn_copy_to_vm(&input),
        "buildVmImage" | "buildDockerImage" => host_fn_build_vm_image(&input),
        "httpPost" | "httpRequest" => host_fn_http_request(&input),
        "elpifyProof" | "verifyProgramExecution" => host_fn_verify_program(&input),
        "protocolApi" | "callProtocolApi" => host_fn_protocol_api(&input),
        "signal" => host_fn_signal(&input),
        // Read a granted secret. The caller identity MUST be the node-authoritative
        // creature bound to this VM (from get_vm_context on the runtime-stamped /
        // gateway-verified vmId), NOT the `creatureId` a guest may put in the
        // request — so `resolve_cached_vm_hierarchy`, never `ctx.creature_id` (which
        // falls back to the claimed field). An unresolvable context denies access.
        "secretGet" => {
            let caller = resolve_cached_vm_hierarchy(packet, &input).creature_id;
            host_fn_secret_get(&caller, &input)
        }
        "createAccess" | "createOwnedAccess" => host_fn_create_access(&input),
        "deleteAccess" | "removeAccess" | "deleteOwnedAccess" | "removeOwnedAccess" => {
            host_fn_delete_access(&input)
        }
        "createStore" | "createOwnedStore" => host_fn_create_store(&input),
        "deleteStore" | "removeStore" | "deleteOwnedStore" | "removeOwnedStore" => {
            host_fn_delete_store(&input)
        }
        "getStore" => host_fn_get_store(&input),
        "listStores" => host_fn_list_stores(&input),
        // List the creatures (members) that have access to a store.
        "listStoreAccess" | "listStoreMembers" | "readMembers" => {
            host_fn_list_store_access(&input)
        }
        "updateStore" => host_fn_update_store(&input),
        "createCreature" | "createOwnedCreature" => host_fn_create_creature(&input),
        "getCreature" => host_fn_get_creature(&input),
        "listCreatures" => host_fn_list_creatures(&input),
        "updateCreature" => host_fn_update_creature(&input),
        "validateSign" => host_fn_validate_sign(&input),
        "transfer" => host_fn_transfer(&input),
        "consumeLock" => host_fn_consume_lock(&input),
        "lockToken" => host_fn_lock_token(&input),
        "createProgram" => host_fn_create_program(&input),
        "deleteProgram" | "deleteOwnedProgram" => host_fn_delete_program(&input),
        // Program CRUD reads — exposed so store/miniapp creatures can fetch a
        // program's record + metadata (e.g. an MCP manifest) and enumerate the
        // programs of a machine. Mirror the creature CRUD reads already present.
        "getProgram" => host_fn_get_program(&input),
        "listPrograms" => host_fn_list_programs(&input),
        "listProgramMachines" => host_fn_list_program_machines(&input),
        "updateProgram" => host_fn_update_program(&input),
        "deployEntity" | "deploy entity" => host_fn_deploy_entity(&input),
        "deleteCreature" | "removeCreature" | "deleteOwnedCreature" | "removeOwnedCreature" => {
            host_fn_delete_creature(&input)
        }
        "signalUser" => host_fn_signal_user(&input),
        "signalGroup" => host_fn_signal_group(&input),
        "lockResource" => host_fn_lock_resource(&input),
        "unlockResource" => host_fn_unlock_resource(&input),
        "vmLog" | "consoleLog" => host_fn_vm_log(&input),
        // Micro ops backed by `Vmm::handle_micro_host_action` (the real DB /
        // signaler / access-control implementations).
        "genId" | "getLink" | "delKey" | "getJson" | "putJson" | "getByPrefix"
        | "hasAccessToStore" | "joinGroup" => host_fn_micro(op, &input),
        // Resource (vm-scoped) store CRUD.
        "createResourceStore" | "createVmOwnedStore" => host_fn_resource_store(&"create".to_string(), &input),
        "updateResourceStore" | "updateVmOwnedStore" => host_fn_resource_store(&"update".to_string(), &input),
        "deleteResourceStore" | "deleteVmOwnedStore" => host_fn_resource_store(&"delete".to_string(), &input),
        "getResourceStore"    | "getVmOwnedStore"    => host_fn_resource_store(&"get".to_string(), &input),
        "listResourceStores"  | "listVmOwnedStores"  => host_fn_resource_store(&"list".to_string(), &input),
        // Resource entities (file blobs etc.).
        "createResourceEntity" => host_fn_resource_entity_create(&input),
        "deleteResourceEntity" => host_fn_resource_entity_delete(&input),
        _ => {
            let packet = json!({
                "key": op,
                "input": input
            });
            wasm_send(packet)
        }
    }
}

/// Generic dispatch into `IVmm::host_action_micro` via the canonical tool path.
pub(crate) fn host_fn_micro(op: &str, input: &JsonValue) -> String {
    match with_global_app(|app| app.tools().vmm().host_action_micro(op, input, 0).0) {
        Some(out) => out,
        None => json!({"ok": false, "error": "vmm not initialised"}).to_string(),
    }
}

/// Read a creature-owned secret on behalf of the calling docker creature.
///
/// The authenticated caller is the container's registered creature id (resolved
/// out-of-band from the VM context — a guest cannot forge it), mirroring the
/// signed `/creatures/secretGet` route for external clients. A caller reading its
/// own secret always succeeds; reading another owner's secret requires an
/// unexpired grant (`SecretGrant::<owner>::<name>::<caller>`). This is how the
/// agent backbone fetches a platform LLM key the operator granted it, instead of
/// the key being baked into the creature image. put/grant/revoke/list stay on the
/// signed routes (the app/operator perform those); a creature only ever reads.
pub(crate) fn host_fn_secret_get(caller: &str, input: &JsonValue) -> String {
    use crate::models::transaction::ITrx;
    use crate::shell::utils::secret_crypto;
    use std::sync::{Arc, Mutex};

    let caller = caller.trim();
    if caller.is_empty() {
        return json!({"ok": false, "error": "caller identity unavailable"}).to_string();
    }
    let name = input["name"].as_str().unwrap_or("").trim().to_string();
    if name.is_empty() || name.contains(':') {
        return json!({"ok": false, "error": "secret name is required"}).to_string();
    }
    let owner = {
        let o = input["owner"].as_str().unwrap_or("").trim();
        if o.is_empty() { caller.to_string() } else { o.to_string() }
    };
    let need_grant = owner != caller;
    let secret_link = format!("Secret::{}::{}", owner, name);
    let grant_link = format!("SecretGrant::{}::{}::{}", owner, name, caller);

    let fetched = with_global_app(|app| {
        let root = app.tools().storage().storage_root().to_string();
        let blob = Arc::new(Mutex::new(String::new()));
        let grant = Arc::new(Mutex::new(String::new()));
        let (b, g) = (blob.clone(), grant.clone());
        let (sl, gl) = (secret_link.clone(), grant_link.clone());
        app.modify_state(
            true,
            Box::new(move |trx: &dyn ITrx| {
                *b.lock().unwrap() = trx.get_link(&sl);
                if need_grant {
                    *g.lock().unwrap() = trx.get_link(&gl);
                }
                Ok(())
            }),
        );
        let blob = blob.lock().unwrap().clone();
        let grant = grant.lock().unwrap().clone();
        (root, blob, grant)
    });
    let (root, blob, grant_raw) = match fetched {
        Some(v) => v,
        None => return json!({"ok": false, "error": "vmm not initialised"}).to_string(),
    };

    if need_grant {
        let expires_at: i64 = grant_raw.trim().parse().unwrap_or(0);
        if expires_at <= 0 || chrono::Utc::now().timestamp_millis() >= expires_at {
            return json!({"ok": false, "error": "access denied: no valid grant for this secret"})
                .to_string();
        }
    }
    if blob.is_empty() {
        return json!({"ok": false, "error": "secret not found"}).to_string();
    }
    let key = match secret_crypto::master_key(&root) {
        Ok(k) => k,
        Err(e) => return json!({"ok": false, "error": e.to_string()}).to_string(),
    };
    match secret_crypto::decrypt(&blob, &key) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(value) => json!({"ok": true, "owner": owner, "name": name, "value": value}).to_string(),
            Err(_) => json!({"ok": false, "error": "stored secret is not valid UTF-8"}).to_string(),
        },
        Err(e) => json!({"ok": false, "error": e.to_string()}).to_string(),
    }
}

// --------------------------------------------------------------------------- //
// Program CRUD reads — the write side (`createProgram`/`deleteProgram`) existed
// but the read side did not, so a store/miniapp creature could not resolve a
// program's record + metadata (e.g. an MCP manifest) or enumerate programs.
// These route through the canonical `IVmm::host_action_program` tool path, the
// same persisted-state mechanism the creature CRUD reads use.
// --------------------------------------------------------------------------- //

/// Dispatch into `IVmm::host_action_program` via the canonical tool path.
fn host_fn_program(op: &str, input: &JsonValue) -> String {
    match with_global_app(|app| app.tools().vmm().host_action_program(op, input, 0).0) {
        Some(out) => out,
        None => json!({"ok": false, "error": "vmm not initialised"}).to_string(),
    }
}

/// List the creatures with access to a store. Input: `{ storeId }`.
pub(crate) fn host_fn_list_store_access(input: &JsonValue) -> String {
    match with_global_app(|app| app.tools().vmm().host_action_store("listAccess", input, 0).0) {
        Some(out) => out,
        None => json!({"ok": false, "error": "vmm not initialised"}).to_string(),
    }
}

/// Fetch a single program's record + metadata. Input: `{ programId }`.
pub(crate) fn host_fn_get_program(input: &JsonValue) -> String {
    host_fn_program("get", input)
}

/// List programs (optionally a page). Input: `{ offset?, count? }`.
pub(crate) fn host_fn_list_programs(input: &JsonValue) -> String {
    host_fn_program("list", input)
}

/// List the programs belonging to a machine creature. Input: `{ machineId }`.
pub(crate) fn host_fn_list_program_machines(input: &JsonValue) -> String {
    host_fn_program("listByMachine", input)
}

/// Update a program's record/metadata. Input: `{ programId, metadata?, ... }`.
pub(crate) fn host_fn_update_program(input: &JsonValue) -> String {
    host_fn_program("update", input)
}

/// Dispatch into `IVmm::host_action_resource_store` via the canonical tool path.
pub(crate) fn host_fn_resource_store(op: &str, input: &JsonValue) -> String {
    match with_global_app(|app| app.tools().vmm().host_action_resource_store(op, input, 0).0) {
        Some(out) => out,
        None => json!({"ok": false, "error": "vmm not initialised"}).to_string(),
    }
}

/// Dispatch into `IVmm::host_action_resource_entity_create`.
pub(crate) fn host_fn_resource_entity_create(input: &JsonValue) -> String {
    match with_global_app(|app| app.tools().vmm().host_action_resource_entity_create(input, 0).0) {
        Some(out) => out,
        None => json!({"ok": false, "error": "vmm not initialised"}).to_string(),
    }
}

/// Dispatch into `IVmm::host_action_resource_entity_delete`.
pub(crate) fn host_fn_resource_entity_delete(input: &JsonValue) -> String {
    match with_global_app(|app| app.tools().vmm().host_action_resource_entity_delete(input, 0).0) {
        Some(out) => out,
        None => json!({"ok": false, "error": "vmm not initialised"}).to_string(),
    }
}

/// Deliver a `signalUser` request from a wasm host-call to the in-process
/// signaler so the target user (typically the requesting CLI client) receives
/// the creature's response packet.
pub(crate) fn host_fn_signal_user(input: &JsonValue) -> String {
    let key = input["key"].as_str().unwrap_or("");
    let user_id = input["userId"].as_str().unwrap_or("");
    if key.is_empty() || user_id.is_empty() {
        return json!({
            "ok": false,
            "error": "signalUser requires key and userId"
        })
        .to_string();
    }
    let packet_str = input["packet"].as_str().unwrap_or("{}");
    let value = serde_json::from_str::<JsonValue>(packet_str).unwrap_or(JsonValue::Null);
    let delivered = crate::drivers::vmm::globals::with_global_app(|app| {
        app.tools().signaler().signal_user(key, user_id, value, true);
    });
    match delivered {
        Some(()) => json!({"ok": true}).to_string(),
        None => json!({"ok": false, "error": "global app not initialised"}).to_string(),
    }
}

/// Deliver a `signalGroup` request from a wasm host-call.
pub(crate) fn host_fn_signal_group(input: &JsonValue) -> String {
    let key = input["key"].as_str().unwrap_or("");
    let group_id = input["groupId"].as_str().unwrap_or("");
    if key.is_empty() || group_id.is_empty() {
        return json!({
            "ok": false,
            "error": "signalGroup requires key and groupId"
        })
        .to_string();
    }
    let packet_str = input["packet"].as_str().unwrap_or("{}");
    let value = serde_json::from_str::<JsonValue>(packet_str).unwrap_or(JsonValue::Null);
    let except: Vec<String> = input
        .get("except")
        .and_then(JsonValue::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let delivered = crate::drivers::vmm::globals::with_global_app(|app| {
        app.tools()
            .signaler()
            .signal_group(key, group_id, value, true, except);
    });
    match delivered {
        Some(()) => json!({"ok": true}).to_string(),
        None => json!({"ok": false, "error": "global app not initialised"}).to_string(),
    }
}

