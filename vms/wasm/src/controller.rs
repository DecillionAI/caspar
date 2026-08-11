//! The wasm VM controller — VmPlugin implementation over the WasmEdge
//! managed runtime.

use std::path::Path;
use std::process::Command;

use serde_json::{json, Value as JsonValue};

use caspar_vm_sdk::host::host;
use caspar_vm_sdk::util::parse_vm_resource_limits;
use caspar_vm_sdk::{VmPlugin, VmPluginMeta};

use crate::runtime::terminate_managed_vm;

pub struct WasmVmController {
    meta: VmPluginMeta,
}

impl WasmVmController {
    pub fn new(meta: VmPluginMeta) -> Self {
        Self { meta }
    }
}

impl VmPlugin for WasmVmController {
    fn meta(&self) -> &VmPluginMeta {
        &self.meta
    }

    /// Spawn a managed wasm VM on its own thread. Every panic and every
    /// wasmedge failure is contained inside that thread, surfaced as a
    /// vmOutput error packet, and the VM slot is always released at the end.
    fn run_vm(&self, packet: &JsonValue) -> Result<JsonValue, String> {
        let ast_path = packet["astPath"].as_str().unwrap_or("").to_string();
        // Reject an unresolved module path up front, before spawning the VM
        // thread. Loading a wasm module from an empty path aborts the whole
        // node inside WasmEdge's C++ loader (see runtime::execute_on_update),
        // so a missing astPath must fail fast as an ordinary error here.
        if ast_path.trim().is_empty() {
            return Err(format!(
                "wasm run requires a module path (astPath); none resolved for machine={} vm={}",
                packet["machineId"].as_str().unwrap_or(""),
                packet["vmId"].as_str().unwrap_or("main")
            ));
        }
        let input = packet["input"].as_str().unwrap_or("{}").to_string();
        let machine_id = packet["machineId"].as_str().unwrap_or("").to_string();
        let vm_id = packet["vmId"].as_str().unwrap_or("main").to_string();
        let limits = parse_vm_resource_limits(packet);

        // Route the signal to this machine's warm VM (one persistent, reused
        // instance per machine, on its own actor thread). The call returns
        // immediately; the VM's response is delivered by its `finalize` vmOutput
        // packet, exactly as the previous per-signal thread did. The warm actor
        // parses `store_id` from the input, applies the per-job timeout, contains
        // panics/errors, honours terminate, and falls back to a cold one-shot run
        // if a warm instance cannot be built.
        crate::runtime::warm_submit(
            &machine_id,
            &vm_id,
            &ast_path,
            input,
            limits.ram_mb,
            limits.max_exec_time_secs,
        );

        Ok(json!({
            "ok": true,
            "runtime": "wasm",
            "machineId": packet["machineId"].as_str().unwrap_or(""),
            "vmId": packet["vmId"].as_str().unwrap_or("main"),
        }))
    }

    fn terminate_vm(&self, packet: &JsonValue) -> Result<JsonValue, String> {
        let machine_id = packet["machineId"].as_str().unwrap_or("");
        if machine_id.is_empty() {
            return Err("machineId is required".to_string());
        }
        terminate_managed_vm(machine_id);
        Ok(json!({"ok": true, "runtime": "wasm", "machineId": machine_id}))
    }

    fn exec_vm(&self, packet: &JsonValue) -> Result<JsonValue, String> {
        let ast_path = packet["astPath"].as_str().unwrap_or("").to_string();
        if ast_path.is_empty() {
            return Err("astPath is required".to_string());
        }
        Ok(json!({"ok": true, "runtime": "wasm", "astPath": ast_path}))
    }

    /// Plan a standalone `runVm` for a deployed wasm entity.
    ///
    /// The generic SDK default omits `astPath`, which would launch the VM with
    /// an empty module path (and abort the node in WasmEdge's loader). The wasm
    /// runtime records the module path on deploy (`setEntityLinksOnDeploy`) at
    /// the link `vmEntityPath::{machineId}::{entityId}`, so resolve it here and
    /// include it in the launch input.
    fn plan_run_entity(&self, ctx: &JsonValue) -> Result<JsonValue, String> {
        let machine_id = ctx["machineId"].as_str().unwrap_or("");
        let entity_id = ctx["entityId"].as_str().unwrap_or("");
        let params = ctx["params"].clone();
        let data = serde_json::to_string(&params).unwrap_or_else(|_| "{}".to_string());

        let mut ast_path = String::new();
        if !machine_id.is_empty() && !entity_id.is_empty() {
            if let Some(h) = host() {
                ast_path = h
                    .state_get(&format!("vmEntityPath::{}::{}", machine_id, entity_id))
                    .trim()
                    .to_string();
            }
        }

        Ok(json!({
            "input": {
                "runtime": self.meta().key,
                "machineId": ctx["machineId"],
                "entityId": ctx["entityId"],
                "standalone": true,
                "vmId": ctx["vmId"],
                "resources": ctx["resources"],
                "astPath": ast_path,
                "data": data,
            },
            "links": [],
        }))
    }

    /// Wasm "image build": run the entity's `build.sh` script (which compiles
    /// the creature sources into `module.wasm`).
    fn build_image(&self, packet: &JsonValue) -> Result<JsonValue, String> {
        let machine_id = packet["machineId"].as_str().unwrap_or("");
        if machine_id.is_empty() {
            return Err("machineId is required".to_string());
        }
        let build_path = packet["imageBuildPath"]
            .as_str()
            .or_else(|| packet["dockerfilePath"].as_str())
            .or_else(|| packet["path"].as_str())
            .unwrap_or("");
        if build_path.is_empty() {
            return Err("build path is required".to_string());
        }
        let script_path = resolve_script_path(build_path, "build.sh")?;
        run_local_build_script(&script_path)?;
        Ok(json!({
            "ok": true,
            "machineId": machine_id,
            "scriptPath": script_path,
            "runtime": "wasm"
        }))
    }
}

fn resolve_script_path(path: &str, default_script_name: &str) -> Result<String, String> {
    let path_ref = Path::new(path);
    if !path_ref.exists() {
        return Err(format!("build path does not exist: {}", path));
    }
    if path_ref.is_file() {
        return Ok(path.to_string());
    }
    let script_path = path_ref.join(default_script_name);
    if !script_path.exists() {
        return Err(format!(
            "required build script not found at {}",
            script_path.to_string_lossy()
        ));
    }
    Ok(script_path.to_string_lossy().to_string())
}

fn run_local_build_script(script_path: &str) -> Result<(), String> {
    let script_ref = Path::new(script_path);
    let script_name = script_ref
        .file_name()
        .ok_or_else(|| format!("invalid build script path: {}", script_path))?
        .to_string_lossy()
        .to_string();
    let cwd = script_ref
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let output = Command::new("bash")
        .arg(script_name)
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("failed to execute wasm build script {}: {}", script_path, e))?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "wasm build script failed (status={}): {}{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    ))
}
