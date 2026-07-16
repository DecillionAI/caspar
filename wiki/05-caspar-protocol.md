# 05 — Caspar Protocol

Caspar exposes several surfaces. This page documents the **signed binary action
protocol** (how clients talk to the node) and the **VM↔host protocols** (how a
running VM communicates with the node): the WASM guest `hostCall` ABI and the
JSON VM packet router. Each is a self-contained section.

Caspar's surfaces:

1. Signed binary action protocol (mutual-TLS TCP + TLS WS) — client ⇄ node.
2. Entity / stream endpoints (`ENTITY_API_PORT`, `VM_API_PORT`).
3. Hashgraph service HTTP endpoints (`BLOCKCHAIN_API_PORT`).
4. Telemetry snapshot HTTP endpoint (`TELEMETRY_API_PORT`).

---

## Binary action protocol — framing

Every message is length-prefixed:

```text
[4 bytes body_len (big-endian)]
[body]
```

The `body` layout:

```text
[4 bytes signature_len][signature]
[4 bytes user_id_len][user_id]
[4 bytes path_len][path]
[4 bytes request_id_len][request_id]
[payload_json]
```

This is exactly what the client CLI builds in `createRequest`
(see [Client CLI](09-client-cli.md)).

---

## Frame tag bytes

A single leading byte tags each frame:

- `0x01` — asynchronous update / signal frame (e.g. `creatures/signal/result`).
- `0x02` — synchronous response frame (carries `request_id` + status + payload).
- `0x03` — request frame.

After processing each inbound frame a client sends a 5-byte flow-control ack
(`00 00 00 01 01`).

---

## Status codes

- `0` success
- `1` action not found
- `2` parse/validation error
- `3` execution error
- `4` auth/authorization failure

---

## Request signatures

Signed actions carry a base64 RSA-SHA256 signature over the exact payload
bytes, verified against the public key registered at login/create. Two padding
schemes are accepted:

- **RSA-PSS** (primary — what the Python SDK, the deploy tooling, and the
  client CLI produce), and
- **RSASSA-PKCS#1 v1.5** (fallback — for mbedTLS-backed clients such as a Godot
  game client using `Crypto.sign`).

Both bind the same key and digest; prefer PSS when the crypto stack supports it.

---

## Consensus routing

Whether an action is ordered through the Babble chain is determined by its
input's `origin` field, **not** by the route name:

- `origin == "global"` → consensus-bound (e.g. `creatures.create`,
  `creatures.createMachine`, `programs.create`, `programs.deploy`, `chains/*`).
  Produces `Adding Transaction → Commit block=N`.
- `origin == ""` → local (reads, `creatures.signal`, dev `login`). No chain
  activity.

---

## Route groups

### Auth
- `GET /auths/getServerPublicKey`, `/auths/getServersMap`

### Users / creatures
- `POST /users/authenticate`, `/users/transfer`, `/users/mint`, `/users/checkSign`
- `POST /users/lockToken`, `/users/consumeLock`, `/users/login`, `/users/create`, `/users/delete`, `/users/update`
- `GET /users/meta`, `/users/get`, `/users/getByUsername`, `/users/find`, `/users/list`

> In the current model these are addressed as `/creatures/*` by the client CLI
> (`/creatures/login`, `/creatures/authenticate`, `/creatures/get`,
> `/creatures/create`, `/creatures/signal`, …).

### Stores
- `POST /stores/addMachine`, `/stores/listMachines`, `/stores/updateProgram`, `/stores/removeMachine`
- `POST /stores/addProgram`, `/stores/removeProgram`, `/stores/addMember`, `/stores/updateMember`
- `POST /stores/updateMemberAccess`, `/stores/updateProgramAccess`, `/stores/getDefaultAccess`, `/stores/readMembers`, `/stores/removeMember`
- `POST /stores/create`, `/stores/join`, `/stores/leave`, `/stores/signal`, `/stores/history`
- `PUT /stores/update`, `DELETE /stores/delete`, `GET /stores/meta`, `/stores/get`, `/stores/read`, `/stores/list`

### Invites
- `POST /invites/create`, `/invites/listStoreInvites`, `/invites/listUserInvites`, `/invites/cancel`, `/invites/accept`, `/invites/decline`

### Machines + Programs
- Machines: `/machines/create`, `/machines/delete`, `/machines/update`, `/machines/myCreated`, `/machines/signal`, `/machines/runProgramEntity`, `/machines/stopProgramEntity`, `/machines/readBuildLogs`, `/machines/readMachineBuilds`, `/machines/deploy`, `/machines/list`, `/machines/listProgramMachines`
- Programs: `/programs/create`, `/programs/deploy`, `/programs/runEntity`, `/programs/stopEntity`, `/programs/update`, `/programs/delete`, `/programs/list`, `/programs/readVmLogs`

### Storage
- `POST /storage/upload`, `/storage/uploadUserEntity`, `/storage/deleteUserEntity`, `/storage/uploadStoreEntity`, `/storage/uploadAppEntity`, `/storage/deleteStoreEntity`, `/storage/download`

### Chains
- `POST /chains/create`, `/chains/createShard`, `/chains/createFromStore`, `/chains/submitBaseTrx`, `/chains/registerNode`

### PC + Misc
- `POST /pc/runPc`, `/pc/execCommand`
- `GET /api/hello`, `/api/time`, `/api/ping`

---

## HTTP surfaces

### Entity API (`ENTITY_API_PORT`)
`/storage/downloadUserEntity`, `/storage/uploadUserEntity`,
`/storage/uploadStoreEntity`, `/storage/uploadAppEntity`,
`/storage/downloadAppEntity`, `/storage/downloadStoreEntity`, `/stream/get`,
`/stream/send`.

### VM stream API (`VM_API_PORT`)
`/stream/send`.

### Telemetry API (`TELEMETRY_API_PORT`, default `9099`)
`GET /telemetry/snapshot`.

### HTTP ingress to VMs
The VMM exposes an HTTP ingress at
`{node instance url}/{creatureId}/{programId}/{entityId}/{vmId}/{path…}`. It
strips the four identity segments, packages the remaining request, and calls
the entity's runtime plugin (`forward_http`). By default the request is wrapped
into a `creatures/signal` and delivered asynchronously (a `202 Accepted`);
runtimes with a long-lived HTTP server inside the VM (e.g. docker) override this
to proxy directly.

---

## The host-call ABI (WASM guest ⇄ host)

Every interaction a WASM creature has with the platform flows through a single
guest import, **`hostCall`**, which takes a JSON request in guest memory and
returns a packed `(offset << 32 | len)` handle to a JSON response. The request
shape is:

```json
{ "op": "<operation>", "input": { ... } }
```

The host recognises these operations (from `vms/wasm/src/host_calls.rs`):

| `op` | Meaning |
|------|---------|
| `output` | Set the VM's execution result (`input.text`). |
| `consoleLog` | Emit a runtime log line (`input.text`). |
| `dbOp` | Low-level raw KV op: `put` / `del` / `get` / `getByPrefix` (namespaced by machine id). |
| `putJson` / `getJson` / `getByPrefix` / `delKey` | Per-VM **JSON transaction** ops on the VM's single lifecycle transaction. |
| `commitTrx` | Commit & reset the per-VM JSON transaction and flush the raw dbOp buffer. |
| `lockResource` / `unlockResource` | Acquire / release a named resource lock. |
| `runVm` / `terminateVm` / `execVm` / `copyToVm` / `buildVmImage` | Orchestrate subordinate VMs (any runtime) via the VM packet router. |
| `httpPost` / `httpRequest` | Perform an outbound HTTP request on behalf of the VM. |
| `verifyProgramExecution` (`elpifyProof`) | Verify a program-execution proof via the provable runtime plugin. |
| *anything else* | Forwarded to the unified host-call dispatcher (`signalUser`, `signalGroup`, …), with `programId`/`machineId` injected. |

Guest exports the host relies on: `malloc` (to allocate the response buffer),
`memory`, and an entry point such as `update`. Legacy single-purpose exports
(`output`, `consoleLog`, `plantTrigger`, `httpPost`, `runDocker`, `execDocker`,
`copyToDocker`, `signalStore`, `trxPut/Get/Del/GetByPrefix`) are still supported
for older creature SDK builds.

---

## The VM packet router (host-side dispatch)

Host-side, every VM operation is a JSON **packet** carrying a canonical `type`
(and usually a `runtime`/`vmType` hint). The router resolves the responsible
plugin and calls the matching method:

| Packet `type` | Plugin method |
|---------------|---------------|
| `runVm` | `run_vm` |
| `terminateVm` | `terminate_vm` |
| `execVm` | `exec_vm` |
| `copyToVm` / `copyFromVm` | `copy_to_vm` / `copy_from_vm` |
| `buildVmImage` | `build_image` |
| `verifyProgramExecution` | `verify_program_execution` |

Runtime resolution order (`registry::resolve_for_packet`): explicit `runtime`
field → `vmType` hint → artifact-extension detection → the default runtime.
For exec/copy/build packets a legacy alias suffix is also honoured
(`execDocker` → `docker`). VMs emit results back to the node by dispatching
`vmOutput` / `vmLog` / `signal` packets through the same channel. The full
plugin contract is in [VM SDK & Plugins](06-vm-sdk-and-plugins.md).

---

## Signals and the async result channel

`creatures.signal` sends a signal to a creature/program entity. When the payload
is a JSON object, the client injects a `correlationId`; the VM handles the
signal and emits its result as a `0x01` frame keyed `creatures/signal/result`
carrying the same `correlationId`, which the client matches to resolve the call.
This makes signals a request/response channel on top of the fire-and-forget
signal bus. `programId`/`entityId` are forwarded at the top level so the node
routes the packet to the program's VM listener (registered under the
`programId`).
