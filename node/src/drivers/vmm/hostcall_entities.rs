//! Translation of `drivers/vmm/hostcall_entities.go` — the CRUD-style host
//! calls available to wasm / javascript / fire runtimes.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{json, Map, Value};

use crate::abstractions::models::core::{StateClosure, ICore};
use crate::abstractions::models::info::IInfo;
use crate::abstractions::models::trx::ITrx;
use crate::abstractions::state::IState;
use crate::core::actor::model::base::info::Info as BaseInfo;
use crate::shell::api::model::{Creature, Store};

use super::vmm::{check_bool, check_i64, check_str, normalize_runtime, is_managed_runtime, Vmm};

fn number_from_input(input: &Value, key: &str, def: i64) -> i64 {
    check_i64(input, key, def)
}

fn bool_from_input(input: &Value, key: &str, def: bool) -> bool {
    check_bool(input, key, def)
}

impl Vmm {
    pub(super) fn handle_creature_crud(&self, op: &str, input: &Value, req_id: i64) -> (String, i64) {
        match op {
            "create" => {
                let mut id = check_str(input, "id", "");
                if id.is_empty() {
                    id = self.gen_id("vm.creature");
                }
                let typ = check_str(input, "type", "agent");
                let username = check_str(input, "username", "");
                let public_key = check_str(input, "publicKey", "");
                let chain_id = check_str(input, "chainId", "main");
                let subchain_id = check_str(input, "subchainId", "main");
                let owner_id = check_str(input, "ownerId", "");
                let balance = number_from_input(input, "balance", 0);
                let id_owned = id.clone();
                let owner_owned = owner_id.clone();
                self.app.modify_state(
                    false,
                    Box::new(move |t: &dyn ITrx| {
                        let c = Creature {
                            id: id_owned.clone(),
                            type_name: typ.clone(),
                            username: username.clone(),
                            public_key: public_key.clone(),
                            chain_id: chain_id.clone(),
                            subchain_id: subchain_id.clone(),
                            owner_id: owner_owned.clone(),
                            balance,
                            ..Default::default()
                        };
                        c.push(t);
                        if !owner_owned.is_empty() {
                            t.put_link(&format!("ownerof::{}::{}", owner_owned, id_owned), "true");
                        }
                        Ok(())
                    }),
                );
                (format!("{{\"ok\":true,\"id\":\"{}\"}}", id), req_id)
            }
            "update" => {
                let id = check_str(input, "id", "");
                if id.is_empty() {
                    return (r#"{"ok":false,"error":"id is required"}"#.into(), req_id);
                }
                let input_owned = input.clone();
                let id_owned = id.clone();
                self.app.modify_state(
                    false,
                    Box::new(move |t: &dyn ITrx| {
                        let mut c = Creature {
                            id: id_owned.clone(),
                            ..Default::default()
                        }
                        .pull(t);
                        if c.id.is_empty() {
                            c.id = id_owned.clone();
                        }
                        if let Some(v) = input_owned.get("type").and_then(Value::as_str) {
                            c.type_name = v.to_string();
                        }
                        if let Some(v) = input_owned.get("username").and_then(Value::as_str) {
                            c.username = v.to_string();
                        }
                        if let Some(v) = input_owned.get("publicKey").and_then(Value::as_str) {
                            c.public_key = v.to_string();
                        }
                        if let Some(v) = input_owned.get("chainId").and_then(Value::as_str) {
                            c.chain_id = v.to_string();
                        }
                        if let Some(v) = input_owned.get("subchainId").and_then(Value::as_str) {
                            c.subchain_id = v.to_string();
                        }
                        if let Some(v) = input_owned.get("ownerId").and_then(Value::as_str) {
                            c.owner_id = v.to_string();
                        }
                        if let Some(v) = input_owned.get("balance").and_then(Value::as_f64) {
                            c.balance = v as i64;
                        }
                        c.push(t);
                        Ok(())
                    }),
                );
                (format!("{{\"ok\":true,\"id\":\"{}\"}}", id), req_id)
            }
            "delete" => {
                let id = check_str(input, "id", "");
                if id.is_empty() {
                    return (r#"{"ok":false,"error":"id is required"}"#.into(), req_id);
                }
                let id_owned = id.clone();
                self.app.modify_state(
                    false,
                    Box::new(move |t: &dyn ITrx| {
                        let c = Creature {
                            id: id_owned.clone(),
                            ..Default::default()
                        }
                        .pull(t);
                        if !c.username.is_empty() {
                            t.del_index("Creature", "username", "id", &c.username);
                        }
                        for col in [
                            "|",
                            "type",
                            "username",
                            "publicKey",
                            "chainId",
                            "subchainId",
                            "ownerId",
                            "balance",
                        ] {
                            t.del_key(&format!("obj::Creature::{}::{}", id_owned, col));
                        }
                        Ok(())
                    }),
                );
                (format!("{{\"ok\":true,\"id\":\"{}\"}}", id), req_id)
            }
            "get" => {
                let id = check_str(input, "id", "");
                if id.is_empty() {
                    return (r#"{"ok":false,"error":"id is required"}"#.into(), req_id);
                }
                let slot = Arc::new(Mutex::new(Creature::default()));
                let slot_clone = slot.clone();
                let id_owned = id.clone();
                self.app.modify_state(
                    true,
                    Box::new(move |t: &dyn ITrx| {
                        let c = Creature {
                            id: id_owned.clone(),
                            ..Default::default()
                        }
                        .pull(t);
                        *slot_clone.lock().unwrap() = c;
                        Ok(())
                    }),
                );
                let creature = slot.lock().unwrap().clone();
                let out = json!({"ok": true, "creature": creature});
                (serde_json::to_string(&out).unwrap_or_default(), req_id)
            }
            "list" => {
                let offset = number_from_input(input, "offset", 0);
                let mut count = number_from_input(input, "count", 100);
                if count <= 0 {
                    count = 100;
                }
                let slot: Arc<Mutex<Vec<Creature>>> = Arc::new(Mutex::new(Vec::new()));
                let slot_clone = slot.clone();
                self.app.modify_state(
                    true,
                    Box::new(move |t: &dyn ITrx| {
                        if let Ok(list) = Creature::all(t, offset, count) {
                            *slot_clone.lock().unwrap() = list;
                        }
                        Ok(())
                    }),
                );
                let creatures = slot.lock().unwrap().clone();
                let out = json!({"ok": true, "creatures": creatures});
                (serde_json::to_string(&out).unwrap_or_default(), req_id)
            }
            _ => (r#"{"ok":false,"error":"unsupported creature op"}"#.into(), req_id),
        }
    }

    pub(super) fn handle_resource_store_crud(
        &self,
        op: &str,
        input: &Value,
        req_id: i64,
    ) -> (String, i64) {
        match op {
            "create" | "update" => {
                let mut store_id = check_str(input, "storeId", "");
                let machine_id = check_str(input, "machineId", "");
                if store_id.is_empty() {
                    store_id = self.gen_id("vm.store");
                }
                let name = check_str(input, "name", &store_id);
                let metadata = input
                    .get("metadata")
                    .cloned()
                    .unwrap_or_else(|| Value::Object(Map::new()));
                let store_id_owned = store_id.clone();
                let machine_id_owned = machine_id.clone();
                let name_owned = name.clone();
                self.app.modify_state(
                    false,
                    Box::new(move |t: &dyn ITrx| {
                        let key = format!("Json::VmResourceStore::{}", store_id_owned);
                        t.put_json(&key, "metadata", &metadata, true)?;
                        let core_meta = json!({
                            "id": store_id_owned.clone(),
                            "name": name_owned.clone(),
                            "machineId": machine_id_owned.clone(),
                        });
                        t.put_json(&key, "core", &core_meta, true)?;
                        if !machine_id_owned.is_empty() {
                            t.put_link(
                                &format!("vmOwnedStore::{}::{}", machine_id_owned, store_id_owned),
                                "true",
                            );
                        }
                        Ok(())
                    }),
                );
                (format!("{{\"ok\":true,\"storeId\":\"{}\"}}", store_id), req_id)
            }
            "delete" => {
                let store_id = check_str(input, "storeId", "");
                if store_id.is_empty() {
                    return (r#"{"ok":false,"error":"storeId is required"}"#.into(), req_id);
                }
                let machine_id = check_str(input, "machineId", "");
                let store_id_owned = store_id.clone();
                let machine_id_owned = machine_id.clone();
                self.app.modify_state(
                    false,
                    Box::new(move |t: &dyn ITrx| {
                        t.del_key(&format!("Json::VmResourceStore::{}::metadata", store_id_owned));
                        t.del_key(&format!("Json::VmResourceStore::{}::core", store_id_owned));
                        if !machine_id_owned.is_empty() {
                            t.del_key(&format!(
                                "link::vmOwnedStore::{}::{}",
                                machine_id_owned, store_id_owned
                            ));
                        }
                        Ok(())
                    }),
                );
                (format!("{{\"ok\":true,\"storeId\":\"{}\"}}", store_id), req_id)
            }
            "get" => {
                let store_id = check_str(input, "storeId", "");
                if store_id.is_empty() {
                    return (r#"{"ok":false,"error":"storeId is required"}"#.into(), req_id);
                }
                let core_slot: Arc<Mutex<Map<String, Value>>> = Arc::new(Mutex::new(Map::new()));
                let meta_slot: Arc<Mutex<Map<String, Value>>> = Arc::new(Mutex::new(Map::new()));
                let core_clone = core_slot.clone();
                let meta_clone = meta_slot.clone();
                let key = format!("Json::VmResourceStore::{}", store_id);
                self.app.modify_state(
                    true,
                    Box::new(move |t: &dyn ITrx| {
                        if let Ok(c) = t.get_json(&key, "core") {
                            *core_clone.lock().unwrap() = c;
                        }
                        if let Ok(m) = t.get_json(&key, "metadata") {
                            *meta_clone.lock().unwrap() = m;
                        }
                        Ok(())
                    }),
                );
                let core = Value::Object(core_slot.lock().unwrap().clone());
                let meta = Value::Object(meta_slot.lock().unwrap().clone());
                let out = json!({"ok": true, "store": {"core": core, "metadata": meta}});
                (serde_json::to_string(&out).unwrap_or_default(), req_id)
            }
            "list" => {
                let machine_id = check_str(input, "machineId", "");
                let prefix = if machine_id.is_empty() {
                    "Json::VmResourceStore::".to_string()
                } else {
                    format!("link::vmOwnedStore::{}::", machine_id)
                };
                let slot: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
                let slot_clone = slot.clone();
                self.app.modify_state(
                    true,
                    Box::new(move |t: &dyn ITrx| {
                        *slot_clone.lock().unwrap() = t.get_by_prefix(&prefix);
                        Ok(())
                    }),
                );
                let stores = slot.lock().unwrap().clone();
                let out = json!({"ok": true, "stores": stores});
                (serde_json::to_string(&out).unwrap_or_default(), req_id)
            }
            _ => (r#"{"ok":false,"error":"unsupported store op"}"#.into(), req_id),
        }
    }

    pub(super) fn handle_resource_entity_create(
        &self,
        input: &Value,
        req_id: i64,
    ) -> (String, i64) {
        let store_id = check_str(input, "storeId", "");
        if store_id.is_empty() {
            return (r#"{"ok":false,"error":"storeId is required"}"#.into(), req_id);
        }
        let entity_type = check_str(input, "entityType", "default");
        let mut entity_id = check_str(input, "entityId", "");
        if entity_id.is_empty() {
            entity_id = self.gen_id("vm.entity");
        }
        let payload = input.get("payload").cloned().unwrap_or_else(|| json!({}));
        let data = check_str(input, "data", "");
        let base_path = PathBuf::from(&self.storage_root)
            .join("vm_stores")
            .join(&store_id)
            .join(&entity_type);
        let _ = fs::create_dir_all(&base_path);
        let path = base_path.join(format!("{}.json", entity_id));
        let _ = fs::write(&path, data.as_bytes());
        let path_str = path.to_string_lossy().into_owned();
        let store_id_owned = store_id.clone();
        let entity_id_owned = entity_id.clone();
        let entity_type_owned = entity_type.clone();
        let path_owned = path_str.clone();
        self.app.modify_state(
            false,
            Box::new(move |t: &dyn ITrx| {
                let key = format!(
                    "Json::VmResourceEntity::{}::{}::{}",
                    store_id_owned, entity_type_owned, entity_id_owned
                );
                t.put_json(&key, "payload", &payload, true)?;
                let meta = json!({
                    "id": entity_id_owned.clone(),
                    "storeId": store_id_owned.clone(),
                    "entityType": entity_type_owned.clone(),
                    "path": path_owned.clone(),
                });
                t.put_json(&key, "meta", &meta, true)?;
                Ok(())
            }),
        );
        (
            format!("{{\"ok\":true,\"entityId\":\"{}\",\"path\":\"{}\"}}", entity_id, path_str),
            req_id,
        )
    }

    pub(super) fn handle_resource_entity_delete(
        &self,
        input: &Value,
        req_id: i64,
    ) -> (String, i64) {
        let store_id = check_str(input, "storeId", "");
        if store_id.is_empty() {
            return (r#"{"ok":false,"error":"storeId is required"}"#.into(), req_id);
        }
        let entity_type = check_str(input, "entityType", "default");
        let entity_id = check_str(input, "entityId", "");
        if entity_id.is_empty() {
            return (r#"{"ok":false,"error":"entityId is required"}"#.into(), req_id);
        }
        let path = PathBuf::from(&self.storage_root)
            .join("vm_stores")
            .join(&store_id)
            .join(&entity_type)
            .join(format!("{}.json", entity_id));
        let _ = fs::remove_file(&path);
        let store_id_owned = store_id.clone();
        let entity_id_owned = entity_id.clone();
        let entity_type_owned = entity_type.clone();
        self.app.modify_state(
            false,
            Box::new(move |t: &dyn ITrx| {
                let key = format!(
                    "Json::VmResourceEntity::{}::{}::{}",
                    store_id_owned, entity_type_owned, entity_id_owned
                );
                t.del_key(&format!("{}::payload", key));
                t.del_key(&format!("{}::meta", key));
                Ok(())
            }),
        );
        (r#"{"ok":true}"#.into(), req_id)
    }

    pub(super) fn handle_vm_chain_request(
        &self,
        op: &str,
        input: &Value,
        req_id: i64,
    ) -> (String, i64) {
        let store_id = check_str(input, "storeId", "");
        let mut receivers: HashMap<String, HashMap<String, bool>> = HashMap::new();
        receivers.insert("*".to_string(), HashMap::new());

        match op {
            "createWorkchain" => {
                let chain_id = self
                    .app
                    .tools()
                    .network()
                    .chain()
                    .create_work_chain(&store_id);
                let payload =
                    json!({"op": op, "chainId": chain_id, "storeId": store_id});
                let payload_bytes = serde_json::to_vec(&payload).unwrap_or_default();
                let owner_id = self.app.owner_id();
                self.app.globe().send_typed_message_on_chain(
                    "main",
                    "chains/vm/request",
                    "vm.chain",
                    payload_bytes,
                    "",
                    &owner_id,
                    receivers,
                    "",
                    &store_id,
                    None,
                    Box::new(|_, _| {}),
                );
                (format!("{{\"ok\":true,\"chainId\":\"{}\"}}", chain_id), req_id)
            }
            "createSubchain" => {
                let work_chain_id = check_str(input, "workChainId", "");
                let mut subchain_id = check_str(input, "subchainId", "");
                let peers: Vec<String> = input
                    .get("peers")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                subchain_id = self.app.tools().network().chain().create_shard_chain(
                    &work_chain_id,
                    &subchain_id,
                    peers.clone(),
                );
                let payload = json!({
                    "op": op,
                    "workChainId": work_chain_id,
                    "subchainId": subchain_id,
                    "peers": peers,
                });
                let owner_id = self.app.owner_id();
                self.app.globe().send_typed_message_on_chain(
                    "main",
                    "chains/vm/request",
                    "vm.chain",
                    serde_json::to_vec(&payload).unwrap_or_default(),
                    "",
                    &owner_id,
                    receivers,
                    "",
                    &store_id,
                    None,
                    Box::new(|_, _| {}),
                );
                (
                    format!(
                        "{{\"ok\":true,\"workChainId\":\"{}\",\"subchainId\":\"{}\"}}",
                        work_chain_id, subchain_id
                    ),
                    req_id,
                )
            }
            op if op.starts_with("delete") => {
                let payload = json!({"op": op, "input": input.clone()});
                let owner_id = self.app.owner_id();
                self.app.globe().send_typed_message_on_chain(
                    "main",
                    "chains/vm/request",
                    "vm.chain",
                    serde_json::to_vec(&payload).unwrap_or_default(),
                    "",
                    &owner_id,
                    receivers,
                    "",
                    &store_id,
                    None,
                    Box::new(|_, _| {}),
                );
                (r#"{"ok":true,"notified":true}"#.into(), req_id)
            }
            _ => (r#"{"ok":false,"error":"unsupported chain op"}"#.into(), req_id),
        }
    }

    pub(super) fn handle_exec_shell_action(&self, input: &Value, req_id: i64) -> (String, i64) {
        let path = check_str(input, "path", "");
        if path.is_empty() {
            return (r#"{"ok":false,"error":"path is required"}"#.into(), req_id);
        }
        let owner = self.app.owner_id();
        let user_id = check_str(input, "userId", &owner);
        let store_id = check_str(input, "storeId", "");
        let signature = check_str(input, "signature", "");
        let packet_id = check_str(input, "packetId", "");

        let secure = match self.app.actor().fetch_secure_action(&path) {
            Some(s) => s,
            None => return (r#"{"ok":false,"error":"action not found"}"#.into(), req_id),
        };
        let payload_raw = input.get("payload").cloned().unwrap_or(Value::Null);
        let payload_bytes = serde_json::to_vec(&payload_raw).unwrap_or_default();
        let parsed = match secure.parse_input("tcp", payload_raw) {
            Ok(p) => p,
            Err(e) => return (
                format!("{{\"ok\":false,\"error\":\"{}\"}}", e.to_string().replace('"', "\\\"")),
                req_id,
            ),
        };
        let result_slot: Arc<Mutex<(i64, Value, Option<String>)>> =
            Arc::new(Mutex::new((0, Value::Null, None)));
        let result_clone = result_slot.clone();
        let user_id_owned = user_id.clone();
        let packet_id_owned = packet_id.clone();
        let signature_owned = signature.clone();
        let payload_bytes_owned = payload_bytes.clone();
        let secure_clone = secure.clone();
        let ip_addr = self.app.ip_addr();
        let info: Arc<dyn IInfo> = Arc::new(BaseInfo::new(&user_id, &store_id));
        let closure: StateClosure = Box::new(move |_state: Arc<dyn IState>| {
            let r = secure_clone.securely_act(
                &user_id_owned,
                &packet_id_owned,
                &payload_bytes_owned,
                &signature_owned,
                parsed.clone(),
                &ip_addr,
                &[true],
            );
            match r {
                Ok((sc, v)) => *result_clone.lock().unwrap() = (sc, v, None),
                Err(e) => *result_clone.lock().unwrap() = (0, Value::Null, Some(format!("{}", e))),
            }
            Ok(())
        });
        self.app.modify_state_securly(false, info, closure);

        let (status, value, err) = {
            let guard = result_slot.lock().unwrap();
            (guard.0, guard.1.clone(), guard.2.clone())
        };
        match err {
            Some(e) => (
                format!("{{\"ok\":false,\"statusCode\":{},\"error\":\"{}\"}}", status, e.replace('"', "\\\"")),
                req_id,
            ),
            None => {
                let out = json!({"ok": true, "statusCode": status, "result": value});
                (serde_json::to_string(&out).unwrap_or_default(), req_id)
            }
        }
    }

    pub(super) fn handle_micro_host_action(
        &self,
        op: &str,
        input: &Value,
        req_id: i64,
    ) -> (String, i64) {
        match op {
            "genId" => {
                let source = check_str(input, "source", "vm.micro");
                let id = self.gen_id(&source);
                (format!("{{\"ok\":true,\"id\":\"{}\"}}", id), req_id)
            }
            "getLink" => {
                let key = check_str(input, "key", "");
                if key.is_empty() {
                    return (r#"{"ok":false,"error":"key is required"}"#.into(), req_id);
                }
                let val_slot = Arc::new(Mutex::new(String::new()));
                let val_clone = val_slot.clone();
                let key_owned = key.clone();
                self.app.modify_state(
                    true,
                    Box::new(move |t: &dyn ITrx| {
                        *val_clone.lock().unwrap() = t.get_link(&key_owned);
                        Ok(())
                    }),
                );
                let v = val_slot.lock().unwrap().clone();
                (format!("{{\"ok\":true,\"value\":\"{}\"}}", v.replace('"', "\\\"")), req_id)
            }
            "delKey" => {
                let key = check_str(input, "key", "");
                if key.is_empty() {
                    return (r#"{"ok":false,"error":"key is required"}"#.into(), req_id);
                }
                if key.starts_with("link::") {
                    return (
                        r#"{"ok":false,"error":"link modifications are not allowed via delKey"}"#.into(),
                        req_id,
                    );
                }
                let key_owned = key.clone();
                self.app.modify_state(
                    false,
                    Box::new(move |t: &dyn ITrx| {
                        t.del_key(&key_owned);
                        Ok(())
                    }),
                );
                (r#"{"ok":true}"#.into(), req_id)
            }
            "createAccess" => {
                let user_id = check_str(input, "userId", "");
                if user_id.is_empty() {
                    return (r#"{"ok":false,"error":"userId is required"}"#.into(), req_id);
                }
                let store_id = check_str(input, "storeId", "");
                if store_id.is_empty() {
                    return (r#"{"ok":false,"error":"storeId is required"}"#.into(), req_id);
                }
                let user_id_owned = user_id.clone();
                let store_id_owned = store_id.clone();
                self.app.modify_state(
                    false,
                    Box::new(move |t: &dyn ITrx| {
                        t.put_link(&format!("onaccess::{}::{}", store_id_owned, user_id_owned), "true");
                        t.put_link(&format!("hasaccess::{}::{}", user_id_owned, store_id_owned), "true");
                        Ok(())
                    }),
                );
                (r#"{"ok":true}"#.into(), req_id)
            }
            "deleteAccess" => {
                let user_id = check_str(input, "userId", "");
                if user_id.is_empty() {
                    return (r#"{"ok":false,"error":"userId is required"}"#.into(), req_id);
                }
                let store_id = check_str(input, "storeId", "");
                if store_id.is_empty() {
                    return (r#"{"ok":false,"error":"storeId is required"}"#.into(), req_id);
                }
                let user_id_owned = user_id.clone();
                let store_id_owned = store_id.clone();
                self.app.modify_state(
                    false,
                    Box::new(move |t: &dyn ITrx| {
                        t.del_key(&format!("link::onaccess::{}::{}", store_id_owned, user_id_owned));
                        t.del_key(&format!("link::hasaccess::{}::{}", user_id_owned, store_id_owned));
                        Ok(())
                    }),
                );
                (r#"{"ok":true}"#.into(), req_id)
            }
            "getJson" => {
                let key = check_str(input, "key", "");
                if key.is_empty() {
                    return (r#"{"ok":false,"error":"key is required"}"#.into(), req_id);
                }
                let path = check_str(input, "path", "");
                let slot: Arc<Mutex<Map<String, Value>>> = Arc::new(Mutex::new(Map::new()));
                let slot_clone = slot.clone();
                let key_owned = key.clone();
                let path_owned = path.clone();
                self.app.modify_state(
                    true,
                    Box::new(move |t: &dyn ITrx| {
                        if let Ok(m) = t.get_json(&key_owned, &path_owned) {
                            *slot_clone.lock().unwrap() = m;
                        }
                        Ok(())
                    }),
                );
                let data = Value::Object(slot.lock().unwrap().clone());
                let out = json!({"ok": true, "data": data});
                (serde_json::to_string(&out).unwrap_or_default(), req_id)
            }
            "putJson" => {
                let key = check_str(input, "key", "");
                if key.is_empty() {
                    return (r#"{"ok":false,"error":"key is required"}"#.into(), req_id);
                }
                let path = check_str(input, "path", "");
                let merge = check_bool(input, "merge", true);
                let obj = input.get("data").cloned().unwrap_or(Value::Null);
                let key_owned = key.clone();
                let path_owned = path.clone();
                self.app.modify_state(
                    false,
                    Box::new(move |t: &dyn ITrx| {
                        let _ = t.put_json(&key_owned, &path_owned, &obj, merge);
                        Ok(())
                    }),
                );
                (r#"{"ok":true}"#.into(), req_id)
            }
            "getByPrefix" => {
                let prefix = check_str(input, "prefix", "");
                let slot: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
                let slot_clone = slot.clone();
                let prefix_owned = prefix.clone();
                self.app.modify_state(
                    true,
                    Box::new(move |t: &dyn ITrx| {
                        *slot_clone.lock().unwrap() = t.get_by_prefix(&prefix_owned);
                        Ok(())
                    }),
                );
                let data = slot.lock().unwrap().clone();
                let out = json!({"ok": true, "data": data});
                (serde_json::to_string(&out).unwrap_or_default(), req_id)
            }
            "hasAccessToStore" => {
                let machine_id = check_str(input, "machineId", "");
                if machine_id.is_empty() {
                    return (r#"{"ok":false,"error":"machineId is required"}"#.into(), req_id);
                }
                let store_id = check_str(input, "storeId", "");
                if store_id.is_empty() {
                    return (r#"{"ok":false,"error":"storeId is required"}"#.into(), req_id);
                }
                let allowed = self
                    .app
                    .tools()
                    .security()
                    .has_access_to_store(&machine_id, &store_id);
                let out = json!({"ok": true, "allowed": allowed});
                (serde_json::to_string(&out).unwrap_or_default(), req_id)
            }
            "signalUser" => {
                let key = check_str(input, "key", "");
                let user_id = check_str(input, "userId", "");
                let packet = check_str(input, "packet", "{}");
                let is_system = check_bool(input, "system", true);
                let value = serde_json::from_str::<Value>(&packet).unwrap_or(Value::Null);
                self.app
                    .tools()
                    .signaler()
                    .signal_user(&key, &user_id, value, is_system);
                (r#"{"ok":true}"#.into(), req_id)
            }
            "signalGroup" => {
                let key = check_str(input, "key", "");
                let group_id = check_str(input, "groupId", "");
                let packet = check_str(input, "packet", "{}");
                let is_system = check_bool(input, "system", true);
                let except: Vec<String> = input
                    .get("except")
                    .and_then(Value::as_array)
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                    .unwrap_or_default();
                let value = serde_json::from_str::<Value>(&packet).unwrap_or(Value::Null);
                self.app
                    .tools()
                    .signaler()
                    .signal_group(&key, &group_id, value, is_system, except);
                (r#"{"ok":true}"#.into(), req_id)
            }
            "joinGroup" => {
                let group_id = check_str(input, "groupId", "");
                let user_id = check_str(input, "userId", "");
                self.app.tools().signaler().join_group(&group_id, &user_id);
                (r#"{"ok":true}"#.into(), req_id)
            }
            _ => (r#"{"ok":false,"error":"unsupported micro op"}"#.into(), req_id),
        }
    }

    pub(super) fn handle_store_crud(&self, op: &str, input: &Value, req_id: i64) -> (String, i64) {
        match op {
            "create" => {
                let mut store_id = check_str(input, "storeId", "");
                let mut creator_id = check_str(input, "creatorId", "");
                if creator_id.is_empty() {
                    creator_id = check_str(input, "userId", "");
                }
                let tag = check_str(input, "tag", "");
                let parent_id = check_str(input, "parentId", "");
                let is_public = bool_from_input(input, "isPublic", false);
                let pers_hist = bool_from_input(input, "persHist", false);
                let metadata = input.get("metadata").cloned().unwrap_or_else(|| json!({}));
                if store_id.is_empty() {
                    store_id = self.gen_id("store");
                }
                let store_id_owned = store_id.clone();
                let creator_id_owned = creator_id.clone();
                let metadata_owned = metadata.clone();
                self.app.modify_state(
                    false,
                    Box::new(move |t: &dyn ITrx| {
                        let store = Store {
                            id: store_id_owned.clone(),
                            tag: tag.clone(),
                            parent_id: parent_id.clone(),
                            is_public,
                            pers_hist,
                            member_count: 1,
                            ..Default::default()
                        };
                        store.push(t);
                        let _ = t.put_json(
                            &format!("StoreMeta::{}", store_id_owned),
                            "metadata",
                            &metadata_owned,
                            true,
                        );
                        if !creator_id_owned.is_empty() {
                            t.put_link(&format!("hasaccess::{}::{}", creator_id_owned, store_id_owned), "true");
                            t.put_link(&format!("creatorof::{}::{}", creator_id_owned, store_id_owned), "true");
                            t.put_link(&format!("onaccess::{}::{}", store_id_owned, creator_id_owned), "true");
                        }
                        Ok(())
                    }),
                );
                let out = json!({"ok": true, "storeId": store_id});
                (serde_json::to_string(&out).unwrap_or_default(), req_id)
            }
            "update" => {
                let store_id = check_str(input, "storeId", "");
                if store_id.is_empty() {
                    return (r#"{"ok":false,"error":"storeId is required"}"#.into(), req_id);
                }
                let input_owned = input.clone();
                let store_id_owned = store_id.clone();
                self.app.modify_state(
                    false,
                    Box::new(move |t: &dyn ITrx| {
                        let mut store = Store {
                            id: store_id_owned.clone(),
                            ..Default::default()
                        }
                        .pull(t);
                        if store.id.is_empty() {
                            return Ok(());
                        }
                        if let Some(v) = input_owned.get("isPublic").and_then(Value::as_bool) {
                            store.is_public = v;
                        }
                        if let Some(v) = input_owned.get("persHist").and_then(Value::as_bool) {
                            store.pers_hist = v;
                        }
                        if let Some(v) = input_owned.get("tag").and_then(Value::as_str) {
                            store.tag = v.to_string();
                        }
                        store.push(t);
                        if let Some(md) = input_owned.get("metadata") {
                            let _ = t.put_json(&format!("StoreMeta::{}", store_id_owned), "metadata", md, true);
                        }
                        Ok(())
                    }),
                );
                (format!("{{\"ok\":true,\"storeId\":\"{}\"}}", store_id), req_id)
            }
            "delete" => {
                let store_id = check_str(input, "storeId", "");
                if store_id.is_empty() {
                    return (r#"{"ok":false,"error":"storeId is required"}"#.into(), req_id);
                }
                let store_id_owned = store_id.clone();
                self.app.modify_state(
                    false,
                    Box::new(move |t: &dyn ITrx| {
                        let store = Store {
                            id: store_id_owned.clone(),
                            ..Default::default()
                        }
                        .pull(t);
                        if !store.id.is_empty() {
                            store.delete(t);
                        }
                        t.del_key(&format!("Json::StoreMeta::{}::metadata", store_id_owned));
                        Ok(())
                    }),
                );
                (format!("{{\"ok\":true,\"storeId\":\"{}\"}}", store_id), req_id)
            }
            "get" => {
                let store_id = check_str(input, "storeId", "");
                if store_id.is_empty() {
                    return (r#"{"ok":false,"error":"storeId is required"}"#.into(), req_id);
                }
                let store_slot = Arc::new(Mutex::new(Store::default()));
                let meta_slot: Arc<Mutex<Map<String, Value>>> = Arc::new(Mutex::new(Map::new()));
                let store_clone = store_slot.clone();
                let meta_clone = meta_slot.clone();
                let store_id_owned = store_id.clone();
                self.app.modify_state(
                    true,
                    Box::new(move |t: &dyn ITrx| {
                        let s = Store {
                            id: store_id_owned.clone(),
                            ..Default::default()
                        }
                        .pull(t);
                        *store_clone.lock().unwrap() = s;
                        if let Ok(m) = t.get_json(&format!("StoreMeta::{}", store_id_owned), "metadata") {
                            *meta_clone.lock().unwrap() = m;
                        }
                        Ok(())
                    }),
                );
                let store = store_slot.lock().unwrap().clone();
                let meta = Value::Object(meta_slot.lock().unwrap().clone());
                let out = json!({"ok": true, "store": store, "metadata": meta});
                (serde_json::to_string(&out).unwrap_or_default(), req_id)
            }
            "list" => {
                let user_id = check_str(input, "userId", "");
                let prefix = if user_id.is_empty() {
                    "obj::Store::".to_string()
                } else {
                    format!("hasaccess::{}::", user_id)
                };
                let slot: Arc<Mutex<Vec<Store>>> = Arc::new(Mutex::new(Vec::new()));
                let slot_clone = slot.clone();
                self.app.modify_state(
                    true,
                    Box::new(move |t: &dyn ITrx| {
                        if let Ok(list) = Store::list(t, &prefix, false, &HashMap::new(), &HashMap::new(), 0, 50) {
                            *slot_clone.lock().unwrap() = list;
                        }
                        Ok(())
                    }),
                );
                let stores = slot.lock().unwrap().clone();
                let out = json!({"ok": true, "stores": stores});
                (serde_json::to_string(&out).unwrap_or_default(), req_id)
            }
            _ => (r#"{"ok":false,"error":"unsupported store op"}"#.into(), req_id),
        }
    }

    /// `wm.handleTerminateVM` — terminates a VM by runtime.
    pub(super) fn handle_terminate_vm(&self, input: &Value, req_id: i64) -> (String, i64) {
        let target_runtime = normalize_runtime(&check_str(input, "runtime", ""));
        if target_runtime.is_empty() {
            return (r#"{"error":1}"#.into(), req_id);
        }
        let target_machine_id = check_str(input, "machineId", "");
        if target_runtime == "docker" {
            let vm_id = check_str(input, "vmId", "");
            if !vm_id.is_empty() {
                let mut entity_id = check_str(input, "entityId", "");
                if entity_id.is_empty() {
                    entity_id = check_str(input, "imageName", "main");
                }
                let container_name = check_str(input, "containerName", "main");
                self.send_to_engine(json!({
                    "type": "terminateVm",
                    "runtime": "docker",
                    "machineId": target_machine_id,
                    "entityId": entity_id,
                    "containerName": container_name,
                    "vmId": vm_id,
                }));
                return ("{}".into(), req_id);
            }
            return ("unsupported runtime".into(), req_id);
        }
        if is_managed_runtime(&target_runtime) {
            self.send_to_engine(json!({
                "type": "terminateVm",
                "runtime": target_runtime,
                "machineId": target_machine_id,
                "vmId": check_vm_id(input),
            }));
            return ("{}".into(), req_id);
        }
        ("unsupported runtime".into(), req_id)
    }

    pub(super) fn handle_check_token_validity(&self, input: &Value, req_id: i64) -> (String, i64) {
        let token_owner_id = check_str(input, "tokenOwnerId", "");
        let token_id = check_str(input, "tokenId", "");
        if token_owner_id.is_empty() || token_id.is_empty() {
            return (r#"{"error":1}"#.into(), req_id);
        }
        let gas_slot = Arc::new(Mutex::new(0i64));
        let gas_clone = gas_slot.clone();
        let token_owner_owned = token_owner_id.clone();
        let token_id_owned = token_id.clone();
        self.app.modify_state(
            true,
            Box::new(move |t: &dyn ITrx| {
                let consumed_key = format!(
                    "Temp::User::{}::consumedTokens::{}",
                    token_owner_owned, token_id_owned
                );
                if t.get_string(&consumed_key) == "true" {
                    return Ok(());
                }
                if let Ok(m) = t.get_json(
                    &format!("Json::User::{}", token_owner_owned),
                    &format!("lockedTokens.{}", token_id_owned),
                ) {
                    if let Some(amount) = m.get("amount").and_then(Value::as_f64) {
                        *gas_clone.lock().unwrap() = amount as i64;
                    }
                }
                Ok(())
            }),
        );
        let gas_limit = *gas_slot.lock().unwrap();
        let out = json!({"gasLimit": gas_limit});
        (serde_json::to_string(&out).unwrap_or_default(), req_id)
    }

    pub(super) fn handle_plant_trigger(&self, input: &Value, req_id: i64) -> (String, i64) {
        use std::thread;
        use std::time::Duration;

        let count = check_i64(input, "count", 0);
        let machine_id = check_str(input, "machineId", "");
        if machine_id.is_empty() {
            return (r#"{"error":1}"#.into(), req_id);
        }
        let tag = check_str(input, "tag", "");
        if tag.is_empty() {
            return (r#"{"error":1}"#.into(), req_id);
        }
        let store_id = check_str(input, "storeId", "");
        if store_id.is_empty() {
            return (r#"{"error":1}"#.into(), req_id);
        }
        let data = check_str(input, "input", "");
        if tag == "alarm" {
            let app = self.app.clone();
            let machine_id_owned = machine_id.clone();
            let store_id_owned = store_id.clone();
            let data_owned = data.clone();
            thread::spawn(move || {
                let machine_id_inner = machine_id_owned.clone();
                let store_id_inner = store_id_owned.clone();
                let data_inner = data_owned.clone();
                let now_ms = super::vmm::now_unix_ms();
                let alarm_time = now_ms + count * 1000;
                app.modify_state(
                    false,
                    Box::new(move |t: &dyn ITrx| {
                        t.put_link(&format!("vmAlarmStoreId::{}", machine_id_inner), &store_id_inner);
                        t.put_link(&format!("vmAlarmData::{}", machine_id_inner), &data_inner);
                        t.put_link(&format!("vmAlarmTime::{}", machine_id_inner), &format!("{}", alarm_time));
                        Ok(())
                    }),
                );
                thread::sleep(Duration::from_secs(count.max(0) as u64));
                let machine_id_drain = machine_id_owned.clone();
                app.modify_state(
                    false,
                    Box::new(move |t: &dyn ITrx| {
                        t.del_key(&format!("link::vmAlarmStoreId::{}", machine_id_drain));
                        t.del_key(&format!("link::vmAlarmData::{}", machine_id_drain));
                        t.del_key(&format!("link::vmAlarmTime::{}", machine_id_drain));
                        Ok(())
                    }),
                );
                if app
                    .tools()
                    .security()
                    .has_access_to_store(&machine_id_owned, &store_id_owned)
                {
                    // Run via the IVmm trait so other implementations are not
                    // mandatory; this matches how Go re-entered itself.
                    app.tools()
                        .vmm()
                        .run_vm(&machine_id_owned, &store_id_owned, &data_owned);
                }
            });
        } else {
            self.app
                .plant_chain_trigger(count, &machine_id, &tag, &machine_id, &store_id, &data);
        }
        ("{}".into(), req_id)
    }

    pub(super) fn handle_signal_store(&self, input: &Value, req_id: i64) -> (String, i64) {
        let machine_id = check_str(input, "machineId", "");
        if machine_id.is_empty() {
            return (r#"{"error":1}"#.into(), req_id);
        }
        let typ_and_temp = check_str(input, "type", "");
        let mut typ = typ_and_temp.clone();
        let mut temp = false;
        if let Some(t) = input.get("temp").and_then(Value::as_bool) {
            temp = t;
        } else {
            let parts: Vec<&str> = typ_and_temp.split('|').collect();
            if let Some(t) = parts.first() {
                typ = t.to_string();
            }
            if parts.len() > 1 {
                temp = parts[1] == "true";
            }
        }
        let store_id = check_str(input, "storeId", "");
        if store_id.is_empty() {
            return (r#"{"error":1}"#.into(), req_id);
        }
        let user_id = check_str(input, "userId", "");
        if user_id.is_empty() {
            return (r#"{"error":1}"#.into(), req_id);
        }
        let data = check_str(input, "data", "");
        // Build the SignalInput action call.
        use crate::shell::api::packets::stores::SignalInput;
        let signal_input = SignalInput {
            typ,
            data,
            store_id: store_id.clone(),
            user_id,
            temp,
        };
        let info: Arc<dyn IInfo> = Arc::new(BaseInfo::new(&machine_id, &store_id));
        let app_for_closure = self.app.clone();
        let closure: StateClosure = Box::new(move |state: Arc<dyn IState>| {
            if let Some(action) = app_for_closure.actor().fetch_action("/stores/signal") {
                let _ = action.act(state, Arc::new(signal_input.clone()));
            }
            Ok(())
        });
        self.app.modify_state_securly(false, info, closure);
        ("{}".into(), req_id)
    }

    pub(super) fn handle_send_message_on_chain(
        &self,
        input: &Value,
        req_id: i64,
    ) -> (String, i64) {
        let chain_id = check_str(input, "chainId", "main");
        let mut key = check_str(input, "msgKey", "");
        if key.is_empty() {
            key = check_str(input, "key", "");
        }
        if key.is_empty() {
            return (r#"{"error":1}"#.into(), req_id);
        }
        let message_type = check_str(input, "messageType", "vm.execute");
        let payload_str = check_str(input, "payload", "{}");
        let signature = check_str(input, "signature", "");
        let owner = self.app.owner_id();
        let user_id = check_str(input, "userId", &owner);
        let reply_to = check_str(input, "replyTo", "");
        let store_id = check_str(input, "storeId", "");
        let receivers = parse_chain_receivers(input);
        let pay = parse_chain_pay_packet(input);
        self.app.globe().send_typed_message_on_chain(
            &chain_id,
            &key,
            &message_type,
            payload_str.into_bytes(),
            &signature,
            &user_id,
            receivers,
            &reply_to,
            &store_id,
            pay,
            Box::new(|_, _| {}),
        );
        ("{}".into(), req_id)
    }

    /// Shared `gen_id(source)` helper using a read-only state transaction.
    pub(super) fn gen_id(&self, source: &str) -> String {
        let slot = Arc::new(Mutex::new(String::new()));
        let slot_clone = slot.clone();
        let storage = self.storage.clone();
        let source_owned = source.to_string();
        self.app.modify_state(
            true,
            Box::new(move |t: &dyn ITrx| {
                *slot_clone.lock().unwrap() = storage.gen_id(t, &source_owned);
                Ok(())
            }),
        );
        let id = slot.lock().unwrap().clone();
        id
    }
}

fn check_vm_id(input: &Value) -> String {
    let v = check_str(input, "vmId", "");
    if v.is_empty() {
        "main".to_string()
    } else {
        v
    }
}

fn parse_chain_receivers(input: &Value) -> HashMap<String, HashMap<String, bool>> {
    let mut receivers: HashMap<String, HashMap<String, bool>> = HashMap::new();
    let Some(nodes) = input.get("receivers").and_then(Value::as_object) else {
        receivers.insert("*".to_string(), HashMap::new());
        return receivers;
    };
    for (node_id, machine_ids_raw) in nodes {
        let mut bucket: HashMap<String, bool> = HashMap::new();
        if let Some(arr) = machine_ids_raw.as_array() {
            for m in arr {
                if let Some(s) = m.as_str() {
                    bucket.insert(s.to_string(), true);
                }
            }
        }
        receivers.insert(node_id.clone(), bucket);
    }
    if receivers.is_empty() {
        receivers.insert("*".to_string(), HashMap::new());
    }
    receivers
}

fn parse_chain_pay_packet(
    input: &Value,
) -> Option<crate::abstractions::models::chain::ChainPayPacket> {
    let Some(pay_obj) = input.get("pay").and_then(Value::as_object) else {
        return None;
    };
    use crate::abstractions::models::chain::ChainPayPacket;
    let mut pay = ChainPayPacket::default();
    let s = |k: &str| pay_obj.get(k).and_then(Value::as_str).map(str::to_string);
    let i = |k: &str| pay_obj.get(k).and_then(Value::as_i64).or_else(|| pay_obj.get(k).and_then(Value::as_f64).map(|v| v as i64));
    if let Some(v) = s("type") { pay.typ = v; }
    if let Some(v) = s("sessionId") { pay.session_id = v; }
    if let Some(v) = s("userId") { pay.user_id = v; }
    if let Some(v) = s("lockId") { pay.lock_id = v; }
    if let Some(v) = s("lockSignature") { pay.lock_signature = v; }
    if let Some(v) = s("storeId") { pay.store_id = v; }
    if let Some(v) = s("vmPayload") { pay.vm_payload = v; }
    if let Some(v) = s("error") { pay.error = v; }
    if let Some(v) = i("amount") { pay.amount = v; }
    if let Some(v) = i("requestedSeconds") { pay.requested_seconds = v; }
    if let Some(v) = i("acceptedSeconds") { pay.accepted_seconds = v; }
    if let Some(v) = i("costPerSecond") { pay.cost_per_second = v; }
    if let Some(arr) = pay_obj.get("machineIds").and_then(Value::as_array) {
        pay.machine_ids = arr.iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
    }
    Some(pay)
}

