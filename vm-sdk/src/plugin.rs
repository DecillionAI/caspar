//! The [`VmPlugin`] trait — the full contract a VM type implements.
//!
//! All methods speak JSON packets (the VMM's native wire shape), so the
//! interface stays stable while individual runtimes evolve. Reasonable
//! defaults are provided for everything except the two operations every
//! runtime must define: [`VmPlugin::run_vm`] and [`VmPlugin::terminate_vm`].

use serde_json::{json, Map, Value};

use crate::meta::VmPluginMeta;

/// A pluggable VM runtime implementation.
pub trait VmPlugin: Send + Sync {
    /// Static descriptor of this runtime (parsed `vm.config.json`).
    fn meta(&self) -> &VmPluginMeta;

    /// One-time hook invoked right after the plugin is registered.
    fn init(&self) {}

    // ── Core lifecycle ────────────────────────────────────────────────────

    /// Launch (or resume) a VM for the given packet.
    fn run_vm(&self, packet: &Value) -> Result<Value, String>;

    /// Stop a VM (suspend by default; runtimes may honour `purge`).
    fn terminate_vm(&self, packet: &Value) -> Result<Value, String>;

    /// Execute a command inside a running VM.
    fn exec_vm(&self, packet: &Value) -> Result<Value, String> {
        let _ = packet;
        Err(format!(
            "exec is not supported for the {} runtime",
            self.meta().key
        ))
    }

    /// Copy a file into a running VM.
    fn copy_to_vm(&self, packet: &Value) -> Result<Value, String> {
        let _ = packet;
        Err(format!(
            "copy_to is not implemented yet for the {} runtime",
            self.meta().key
        ))
    }

    /// Copy a file out of a running VM.
    fn copy_from_vm(&self, packet: &Value) -> Result<Value, String> {
        let _ = packet;
        Err(format!(
            "copy_from is not implemented yet for the {} runtime",
            self.meta().key
        ))
    }

    /// Build the deployable image/module for an entity of this runtime.
    fn build_image(&self, packet: &Value) -> Result<Value, String> {
        let _ = packet;
        Ok(json!({"ok": true, "runtime": self.meta().key, "build": "noop"}))
    }

    // ── Aliased lifecycle verbs (kept for controller-level API parity) ────

    fn create(&self, packet: &Value) -> Result<Value, String> {
        self.run_vm(packet)
    }
    fn start(&self, packet: &Value) -> Result<Value, String> {
        self.run_vm(packet)
    }
    fn stop(&self, packet: &Value) -> Result<Value, String> {
        self.terminate_vm(packet)
    }
    fn resume(&self, packet: &Value) -> Result<Value, String> {
        self.run_vm(packet)
    }
    fn pause(&self, packet: &Value) -> Result<Value, String> {
        self.terminate_vm(packet)
    }

    // ── Snapshot restore ──────────────────────────────────────────────────

    /// Restore one previously-running VM from a node snapshot entry.
    /// Restorable runtimes relaunch the VM; others acknowledge and skip.
    fn restore(&self, snapshot_entry: &Value) -> Result<Value, String> {
        if self.meta().restorable {
            self.run_vm(snapshot_entry)
        } else {
            Ok(json!({"ok": true, "runtime": self.meta().key, "skipped": true}))
        }
    }

    // ── Runtime resolution ────────────────────────────────────────────────

    /// Whether this plugin claims a run request given the packet's runtime
    /// hints and the resolved artifact path.
    fn detect(&self, runtime_hint: &str, artifact_path: &str) -> bool {
        self.meta().matches_key(runtime_hint) || self.meta().matches_artifact(artifact_path)
    }

    /// Resolve a live VM instance name (e.g. a container name) from a source
    /// IP for gateway connection identification.
    fn identify_instance_by_ip(&self, ip: &str) -> Option<String> {
        let _ = ip;
        None
    }

    // ── Program-shell integration plans ───────────────────────────────────
    //
    // The node's program API (deploy / runEntity / stopEntity) never encodes
    // per-runtime behaviour. Instead it asks the plugin for a *plan* — plain
    // JSON describing the packet to send and the state links to write/read —
    // and executes that plan inside its own transaction.

    /// Plan a standalone `runVm` for a deployed program entity.
    ///
    /// `ctx`: `{ machineId, programId, entityId, vmId, resources, params }`.
    /// Returns `{ input: {..runVm input..}, links: [[key, value], ...] }`.
    fn plan_run_entity(&self, ctx: &Value) -> Result<Value, String> {
        let params = ctx["params"].clone();
        let data = serde_json::to_string(&params).unwrap_or_else(|_| "{}".to_string());
        Ok(json!({
            "input": {
                "runtime": self.meta().key,
                "machineId": ctx["machineId"],
                "entityId": ctx["entityId"],
                "standalone": true,
                "vmId": ctx["vmId"],
                "resources": ctx["resources"],
                "data": data,
            },
            "links": [],
        }))
    }

    /// Plan a `terminateVm` for a running program entity.
    ///
    /// `ctx`: `{ machineId, programId, entityId, vmId }`.
    /// Returns `{ input: {..terminateVm input..},
    ///            links: [{field, key, required}, ...] }` where each `links`
    /// entry asks the caller to read the state link `key` and place its value
    /// into `input[field]` (failing when `required` and the link is empty).
    fn plan_stop_entity(&self, ctx: &Value) -> Result<Value, String> {
        Ok(json!({
            "input": {
                "runtime": self.meta().key,
                "machineId": ctx["machineId"],
                "entityId": ctx["entityId"],
                "vmId": ctx["vmId"],
            },
            "links": [],
        }))
    }

    /// Translate a host-call `terminateVm` input into the typed terminate
    /// packet dispatched to the packet router.
    fn build_terminate_request(&self, input: &Value) -> Result<Value, String> {
        let vm_id = input["vmId"].as_str().unwrap_or("").trim();
        let vm_id = if vm_id.is_empty() { "main" } else { vm_id };
        Ok(json!({
            "type": "terminateVm",
            "runtime": self.meta().key,
            "machineId": input["machineId"].as_str().unwrap_or(""),
            "vmId": vm_id,
        }))
    }

    // ── Inbound HTTP forwarding ───────────────────────────────────────────
    //
    // The VMM exposes an HTTP ingress at
    //   `{node instance url}/{creatureId}/{programId}/{entityId}/{path…}`
    // It strips the three identity segments, packages the remaining request,
    // and calls [`VmPlugin::forward_http`] on the entity's runtime plugin.

    /// Forward an inbound HTTP request to the entity's VM.
    ///
    /// `packet`:
    /// `{ creatureId, programId, entityId, machineId, vmId, containerName,
    ///    method, path, query, headers: {..}, bodyBase64 }`
    ///
    /// where `path` is the remainder of the request URL *after* the
    /// `creatureId/programId/entityId` prefix, `query` the raw query string,
    /// and `bodyBase64` the base64-encoded request body.
    ///
    /// The response is `{ ok, status, headers: {..}, body | bodyBase64 }`.
    ///
    /// The default implementation is the generic fallback every runtime
    /// supports: it packages the request into a `creatures/signal` addressed
    /// to the entity and hands it to the node's signalling API, so the VM
    /// handles the request on its next run. Because signalling is
    /// asynchronous the response is a `202 Accepted` acknowledgement.
    /// Runtimes that keep a long-lived HTTP server *inside* the VM (e.g.
    /// docker) override this to proxy the request straight to that server and
    /// return its real HTTP response.
    fn forward_http(&self, packet: &Value) -> Result<Value, String> {
        forward_http_via_signal(packet)
    }

    // ── Optional capabilities ─────────────────────────────────────────────

    /// Verify a program execution proof (provable runtimes only).
    /// Packet: `{ masmPath, inputs, outputs, proof }`.
    fn verify_program_execution(&self, packet: &Value) -> Result<Value, String> {
        let _ = packet;
        Err(format!(
            "program execution verification is not supported by the {} runtime",
            self.meta().key
        ))
    }
}

/// The generic HTTP-forwarding fallback shared by every runtime that does not
/// run a long-lived HTTP server inside its VM.
///
/// The packaged request is wrapped into a `stores::Send`-shaped
/// `creatures/signal` addressed to the entity's program and delivered through
/// the node's signalling API (`signalUser`). The node's per-machine signal
/// listener picks it up and runs the VM for the target entity, which handles
/// the request as an ordinary signal. Delivery is asynchronous, so the caller
/// receives a `202 Accepted` acknowledgement rather than the VM's own output.
pub fn forward_http_via_signal(packet: &Value) -> Result<Value, String> {
    let host = crate::host::host_or_err()?;

    let program_id = packet["programId"].as_str().unwrap_or("").trim().to_string();
    if program_id.is_empty() {
        return Err("forward_http requires a programId".to_string());
    }
    let entity_id = packet["entityId"].as_str().unwrap_or("").trim().to_string();

    // The packaged HTTP request delivered to the VM as the signal payload.
    let http = json!({
        "kind": "httpRequest",
        "creatureId": packet["creatureId"].as_str().unwrap_or(""),
        "programId": program_id,
        "entityId": entity_id,
        "method": packet["method"].as_str().unwrap_or("GET"),
        "path": packet["path"].as_str().unwrap_or("/"),
        "query": packet["query"].as_str().unwrap_or(""),
        "headers": packet["headers"].clone(),
        "bodyBase64": packet["bodyBase64"].as_str().unwrap_or(""),
    });

    // `stores::Send`-shaped signal the VMM's per-machine listener understands:
    // `action: "single"` + the target entity id, with the request in `data`.
    let signal = json!({
        "action": "single",
        "entityId": entity_id,
        "data": http.to_string(),
    });

    let call = json!({
        "op": "signalUser",
        "programId": program_id,
        "input": {
            "key": "creatures/signal",
            "userId": program_id,
            "packet": signal.to_string(),
        }
    });

    let raw = host.unified_host_call(&call);
    let acked = serde_json::from_str::<Value>(&raw)
        .ok()
        .and_then(|v| v["ok"].as_bool())
        .unwrap_or(false);

    let body = json!({
        "ok": acked,
        "forwarded": "signal",
        "programId": program_id,
        "entityId": entity_id,
    })
    .to_string();

    Ok(json!({
        "ok": acked,
        "status": if acked { 202 } else { 502 },
        "headers": { "content-type": "application/json" },
        "body": body,
    }))
}

/// Convenience: merge extra fields into a plan `input` object.
pub fn merge_input(base: &mut Value, extra: Map<String, Value>) {
    if let Some(obj) = base.as_object_mut() {
        for (k, v) in extra {
            obj.insert(k, v);
        }
    }
}
