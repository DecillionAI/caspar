# Docker-host bridge gateway

The **docker-host bridge gateway** is the single, unified channel between
docker-based creature programs (e.g. the Davinci agent and each of its tools,
each deployed as its own `docker` creature) and the Caspar node.

A docker creature is started sandboxed (gVisor / `runsc`) on the gateway docker
network with **no other route to the outside world**. Its only egress/ingress is
one long-lived TCP connection to this gateway. Over that connection the
container does *everything*:

* persists through the node's DB / storage host functions,
* makes outbound HTTP calls through the node's HTTP host function,
* signals sibling creatures,
* spawns / terminates sibling VMs,

and it **receives signals** from other creatures pushed over the same socket.

This replaces the old design where containers reached the node over the external
client TCP API (port 8074) with their own RSA-signed sessions.

## Where it lives — and how it's reached

The gateway is part of the VMM driver's network module, kept self-contained:

```
node/src/drivers/vmm/network/docker_host/
├── mod.rs          # re-exports DockerHostGateway + ContainerIdentity
├── protocol.rs     # chunked wire framing + reassembly (unit-tested)
├── connection.rs   # per-connection state + the live-connection registry
├── dispatch.rs     # maps a request onto the unified host-function surface
├── gateway.rs      # the owning DockerHostGateway instance
└── server.rs       # per-connection I/O loop
```

**Object-oriented, no statics.** There is no process-wide gateway. A single
`DockerHostGateway` instance is **owned by the `Vmm` driver** (a field on the
`Vmm` struct) and is reached only through the canonical
`ICore → tools() → vmm()` object graph, like the rest of the VMM subsystem. The
`IVmm` trait exposes the gateway surface:

```rust
fn start_docker_gateway(&self, port: i64);
fn register_vm_container(&self, container_name, vm_id, creature_id, program_id, machine_id);
fn unregister_vm_container(&self, container_name);
fn identify_container_by_ip(&self, ip) -> Option<(vm_id, creature_id, program_id, machine_id)>;
fn push_signal_to_machine(&self, machine_id, key, data) -> usize;
```

It reuses the proven concurrency model of the federation `netserver`: each
connection is pinned to a dedicated I/O thread that owns its `TcpStream`; a 20 ms
read timeout lets that thread interleave inbound reads with draining an outbound
MPSC queue, so signal pushes and host-call responses never block on, or deadlock
against, the read path.

## Security: identity is derived from the docker source IP

A container must **never** be trusted to declare its own `vmId`/`creatureId`/
`programId` — a malicious container could otherwise claim another VM's identity
and read/write its data. Nor does the container hold any secret to present.
Instead the node identifies a connection from facts only docker controls:

* When the node launches a docker creature it records the binding
  `container name → (vm_id, creature_id, program_id, machine_id)` via
  `tools().vmm().register_vm_container(...)`. The container is given **no
  identity and no secret** — only the gateway address.
* On `HELLO` the node takes the connection's docker-network **source IP** and
  calls `tools().vmm().identify_container_by_ip(ip)`, which asks the docker
  daemon (bollard `list_containers`) *which container currently owns that IP*,
  then maps the returned container name to the registered identity. That
  identity is pinned to the connection for its lifetime and stamped onto every
  request; any identity fields in a request are ignored.
* The binding is dropped (`unregister_vm_container`) when the VM terminates or
  times out.

This is spoof-resistant: a sandboxed container cannot forge another container's
bridge source IP, and it is docker — not the container — that reports the
IP→container mapping. It gives docker creatures the **same security posture** the
in-process wasm runtime already has, where `host_call` stamps the node-created
runtime's own `machine_id`/`vm_id` and never trusts guest-supplied identity.

## Lifecycle

1. The node starts a docker creature (`DockerVmController::run_vm`), records the
   `container name → identity` binding, and injects only:
   * `CASPAR_GATEWAY_HOST` / `CASPAR_GATEWAY_PORT` — where to dial.

   The container is also given `--add-host host.docker.internal:host-gateway`
   so it can resolve the node host from inside the bridge network.

2. After start/init the container connects and sends an empty `HELLO`. The node
   resolves the connection's docker-network source IP → container → identity,
   registers the connection, and replies `WELCOME` echoing that identity
   (read-only) so the container can address replies without ever declaring its
   own id.

3. The container streams `REQUEST` host-calls and receives `RESPONSE`s. The node
   stamps the connection's resolved identity onto every request, so a container
   can never act in another creature's namespace.

4. When another creature signals this machine, the node's machine listener
   pushes the signal onto the container's connection as a `SIGNAL` frame instead
   of cold-spawning a new VM.

5. On disconnect the connection is removed from the registry and the session is
   revoked.

## Wire protocol (chunked)

Every frame is length-delimited on the wire:

```
[u32 BE frame_len][frame body]

frame body:
[u8 op][u64 BE message_id][u64 BE correlation_id][u32 BE seq][u32 BE total][chunk]
```

* `op` — message kind (see below).
* `message_id` — unique per logical message; all chunks share it.
* `correlation_id` — request id echoed on the matching `RESPONSE`.
* `seq` / `total` — chunk index (0-based) and total chunk count.

A logical message larger than `MAX_CHUNK` (64 KiB) is split across several frames
that share `message_id`; the receiver reassembles them (out-of-order tolerant).
A message that fits in one chunk uses `seq = 0`, `total = 1`.

### Opcodes

| op   | name     | direction          | payload                                                  |
|------|----------|--------------------|----------------------------------------------------------|
| 0x01 | HELLO    | container → node   | `{}` (empty — identity is derived from the source IP)    |
| 0x02 | WELCOME  | node → container   | `{ok, sessionId, vmId, machineId, creatureId, programId}`|
| 0x10 | REQUEST  | container → node   | `{op, input}`                             |
| 0x11 | RESPONSE | node → container   | host-function result (JSON)               |
| 0x20 | SIGNAL   | node → container   | `{key, data}`                             |
| 0x30 | PING     | container → node   | `{}`                                       |
| 0x31 | PONG     | node → container   | `{ok}`                                     |
| 0x40 | ERROR    | node → container   | `{ok:false, error}` (terminates session)  |

### Host calls

A `REQUEST` payload is `{ "op": "<host function>", "input": { ... } }`. `op` is
any name accepted by `handle_unified_host_call` — the exact entry point the
in-process wasm/elpian runtimes use — e.g. `dbOp`, `httpRequest`, `signal`,
`signalUser`, `signalGroup`, `runVm`, `createStore`, `getStore`, … The result is
returned verbatim as the `RESPONSE` payload.

## Configuration

| env var                              | default               | meaning                                  |
|--------------------------------------|-----------------------|------------------------------------------|
| `DOCKER_HOST_GATEWAY_PORT`           | `8079`                | listen port (≤ 0 disables the gateway); started via `tools().vmm().start_docker_gateway(port)` |
| `DOCKER_HOST_GATEWAY_ADVERTISE_HOST` | `host.docker.internal`| host injected into containers to dial    |

Per-container env injected by the node: `CASPAR_GATEWAY_HOST`,
`CASPAR_GATEWAY_PORT`, and `CASPAR_VM_ID` (for log readability only — never
trusted for auth; identity is derived from the connection's docker source IP).

## Clients

* Python: `davinci/caspar_bridge.py` (`CasparBridgeClient`, `bridge_from_env()`),
  shipped into every tool image by the deploy harness. Used by the Davinci
  creature and the tool runtime.
