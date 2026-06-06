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

## Where it lives

The gateway is part of the VMM driver's network module, kept self-contained:

```
node/src/drivers/vmm/network/docker_host/
├── mod.rs          # public API: start(), push_signal_to_machine/vm()
├── protocol.rs     # chunked wire framing + reassembly (unit-tested)
├── connection.rs   # per-connection state + the live-connection registry
├── dispatch.rs     # maps a request onto the unified host-function surface
└── server.rs       # TCP listener + per-connection I/O loop
```

It reuses the proven concurrency model of the federation `netserver`: each
connection is pinned to a dedicated I/O thread that owns its `TcpStream`; a 20 ms
read timeout lets that thread interleave inbound reads with draining an outbound
MPSC queue, so signal pushes and host-call responses never block on, or deadlock
against, the read path.

## Lifecycle

1. The node starts a docker creature (`DockerVmController::run_vm`) and injects:
   * `CASPAR_GATEWAY_HOST` / `CASPAR_GATEWAY_PORT` — where to dial, and
   * `CASPAR_VM_ID` / `CASPAR_MACHINE_ID` / `CASPAR_PROGRAM_ID` /
     `CASPAR_CREATURE_ID` — the container's identity.

   The container is also given `--add-host host.docker.internal:host-gateway`
   so it can resolve the node host from inside the bridge network.

2. After start/init the container connects and sends `HELLO` with its identity.
   The node registers the connection in the live-connection registry and replies
   `WELCOME`.

3. The container streams `REQUEST` host-calls and receives `RESPONSE`s. The node
   stamps the connection's *verified* identity onto every request, so a
   container can never act in another creature's namespace.

4. When another creature signals this machine, the node's machine listener
   pushes the signal onto the container's connection as a `SIGNAL` frame instead
   of cold-spawning a new VM.

5. On disconnect the connection is removed from the registry.

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

| op   | name     | direction          | payload                                   |
|------|----------|--------------------|-------------------------------------------|
| 0x01 | HELLO    | container → node   | `{vmId, machineId, programId, creatureId}`|
| 0x02 | WELCOME  | node → container   | `{ok, sessionId, vmId}`                   |
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
| `DOCKER_HOST_GATEWAY_PORT`           | `8079`                | listen port (≤ 0 disables the gateway)   |
| `DOCKER_HOST_GATEWAY_ADVERTISE_HOST` | `host.docker.internal`| host injected into containers to dial    |

## Clients

* Python: `davinci/caspar_bridge.py` (`CasparBridgeClient`, `bridge_from_env()`),
  shipped into every tool image by the deploy harness. Used by the Davinci
  creature and the tool runtime.
