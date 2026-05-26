use crate::drivers::vmm::prelude::*;

/// Unified `runVm` host op. Any caller (a wasm creature, a fire VM
/// host callback, etc.) can invoke any backend runtime from here —
/// docker, fire, elpify, elpian, wasm — by setting the `runtime`
/// field. The result the caller gets back depends on the runtime:
///
/// - `elpify`: the host runs the MASM program to completion and
///   returns `{ok, outputs, proof}` synchronously, so the caller can
///   pipe the proof straight into `elpifyProof` and ride consensus
///   on-chain.
/// - `elpian`: the host runs the elpian AST to completion and returns
///   the result synchronously (matches Elpify's
///   "leave-running-not-needed" semantics).
/// - `fire` / `docker` / `wasm`: the host spawns the VM (long-running
///   or one-shot) and returns its `vmId` / `machineId` immediately
///   so the caller can address it later through `execVm`,
///   `terminateVm`, signals, etc.
///
/// All of this routing already exists in `route_vm_packet`'s
/// `dispatch_run_vm_packet`; this host fn just wraps the input in the
/// `{type:"runVm", ...}` shape that the unified dispatcher expects
/// and delegates.
pub(crate) fn host_fn_run_vm(input: &JsonValue) -> String {
    let mut packet = input.clone();
    if let JsonValue::Object(map) = &mut packet {
        map.insert("type".to_string(), JsonValue::String("runVm".to_string()));
    }
    crate::drivers::vmm::dispatch_packet(&packet)
}
