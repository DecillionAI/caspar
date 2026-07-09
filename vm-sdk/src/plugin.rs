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

/// Convenience: merge extra fields into a plan `input` object.
pub fn merge_input(base: &mut Value, extra: Map<String, Value>) {
    if let Some(obj) = base.as_object_mut() {
        for (k, v) in extra {
            obj.insert(k, v);
        }
    }
}
