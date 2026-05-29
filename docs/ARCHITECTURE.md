# Architecture 🧠

> Updated: **2026-05-29** — current Rust node (`caspar-node` v0.1.0).

Caspar is a single Rust binary that runs several concurrent services and hosts
user logic as WebAssembly **creatures**. This document describes the subsystems
and the mechanisms that connect them.

## 1) Runtime Topology

```text
        mutual-TLS TCP / WebSocket clients
                       │
                ┌──────▼───────┐
                │ Action Router │  signed-packet validation · guards · auth
                └──────┬───────┘
        ┌──────────────┼───────────────────────────┐
        ▼              ▼                             ▼
  Core Txn        Store Layer                  Storage Layer
  (Babble)                                     (RocksDB · QuestDB)
        │              │
        ▼              ▼
 Hashgraph Chain   VM Packet Router  ── runtime-agnostic dispatch ──┐
                                                                    ▼
                        wasm · docker · elpify · elpian · javascript · firecracker
```

## 2) Startup Composition (`node/src/main.rs`)

The entrypoint wires adapters/tools, installs action routes and pluggers, then
starts concurrent services:

- **pprof** profiling HTTP server (`:9999`, Rust-native).
- **Telemetry** HTTP server — `GET /telemetry/snapshot` (`TELEMETRY_API_PORT`,
  default `:9099`).
- **TLS TCP** client server (`CLIENT_TCP_API_PORT`).
- **TLS WebSocket** client server (`CLIENT_WS_API_PORT`).
- **Federation** transport (`FEDERATION_API_PORT`).
- **Babble chain** service (`BLOCKCHAIN_API_PORT`).

On boot the node also restores in-memory VMM signaler listeners (one per unique
`machine_id`) so previously-deployed creatures keep receiving signals.

## 3) Core Subsystems

- **Action Router** (`node/src/shell/api`) — validates the signed packet,
  resolves the target route, applies guard-based authorisation, and dispatches
  to a handler. Action handlers live in `shell/api/actions` (`auth`, `creature`,
  `program`, …).
- **Core transactions** (`node/src/core`) — transaction lifecycle, the
  `ICore` context, callbacks, and update propagation. Actions whose input
  declares `origin == "global"` are consensus-bound; `origin == ""` runs locally.
- **Hashgraph chain** (`node/src/drivers/network/chain`) — an embedded Babble
  (Rust) implementation. The node registers a `commit_handler` that receives
  ordered blocks of transactions and fans them out to application state.
- **VM Manager / VM Packet Router** (`node/src/drivers/vmm`) — a single
  `dispatch_packet` / `route_vm_packet` path drives all six runtimes. The
  controllers live in `drivers/vmm/controllers` (`wasm`, `docker`, `elpify`,
  `elpian`, `javascript`, `fire`).
- **appengine** — the execution engine that hosts the managed (non-wasm)
  runtimes; the node exposes a local socket the engine connects to. WASM
  creatures themselves run in-process via WasmEdge.
- **Storage drivers** (`node/src/drivers/storage.rs`) — RocksDB for creature and
  chain state (via the `TrxWrapper` abstraction) and QuestDB over the PostgreSQL
  wire protocol for telemetry/time-series data.
- **Telemetry** (`node/src/telemetry`) — cached snapshot served over HTTP and
  consumed by `casparctl stats`.

## 4) Creature Programming Model

A **creature** is the fundamental unit of application logic:

- **Code** — a `.wasm` binary loaded into a WasmEdge instance.
- **State** — a per-creature RocksDB namespace, reached through host calls
  (`putJson`, `getJson`, `getByPrefix`, `delKey`). State persists across signals.
- **Signals** — `signalGroup` fans a message to elected validators / group
  members without passing through consensus (sub-consensus fan-out).
- **VM ops** — `buildVmImage`, `runVm`, `execVm`, `copyToVm`, `terminateVm`
  let a creature orchestrate subordinate VMs in any runtime.
- **Transactions** — `commitTrx` checkpoints the per-VM transaction.

### Per-VM persistent transaction

Each active VM holds a single `TrxWrapper` (one RocksDB transaction) for its
lifetime, stored in a per-VM registry
(`Mutex<HashMap<VmId, Arc<dyn ITrx>>>`) inside the `ICore` context. All
key-value mutations within a creature signal commit atomically — on an explicit
`commitTrx` or on VM finalisation — eliminating write amplification and never
exposing partial state between host calls.

## 5) Multi-Runtime VM Router

| Runtime | Purpose |
|---------|---------|
| `wasm` | WasmEdge-hosted secondary WASM modules |
| `docker` | OCI container images via the Bollard client |
| `elpify` | Miden STARK VM (proof-generating MASM execution) |
| `elpian` | native Rust VM for high-speed computation |
| `javascript` | JavaScript runtime for script-based units |
| `firecracker` | micro-VM hypervisor for hardware-isolated workloads |

`task_graph.rs` is runtime-agnostic: each op injects a canonical `"type"` field
and delegates to `dispatch_packet`. The router is the single locus of
`docker` / `fire` / `wasm` / `elpify` / `elpian` / `javascript` branching, so
adding a runtime is a one-file change.

## 6) elpify-chain (STARK validator election)

The elpify-chain creature runs a five-phase commit-reveal Proof-of-Stake
election entirely inside WASM:

1. **Stake** — record `(id, stake)`.
2. **Commit** — validators submit `h = H(s ‖ n)`.
3. **Reveal** — validators submit `(s, n)`; the creature checks the hash and
   accumulates VRF input.
4. **electionTick** — a MASM program runs in the elpify VM; the Miden prover
   (the `elpify-lang` crate) emits a STARK proof; winners are selected and the
   proof is broadcast via `signalGroup`.
5. **Validator consensus** — electors verify the succinct proof and finalise.

Proving is asymptotically `O(n log² n)` and verification `O(log² n)`, so adding
a validator is far cheaper than generating the proof.

## 7) Federation

Each node runs a federation transport (`FEDERATION_API_PORT`) that validates the
signature and origin of inbound packets against a known-origins registry before
admitting them to the local action router. Outbound calls propagate creature
updates and chain events to registered peers — enabling cross-deployment
creature composition and cross-shard event delivery without replicating consensus
history.

## 8) Sharding

A shard is a self-contained three-node Babble consensus group. The chain API
exposes `chains/createShard` and `chains/registerNode`; each shard runs its own
Babble instance with no cross-shard coordination, so aggregate throughput is
additive (`TPS_network = S × TPS_shard`).

## 9) Security Model 🔒

- Signed request packets (identity + signature validation) over mutual TLS
  (Rustls + ring); identity keys are secp256k1 (`k256`), server crypto RSA + SHA-2.
- Guard-based authorisation on action routes.
- Store / member / access checks before state mutation.
- Federated packets validated against known origins.
- WASM provides capability-based logical isolation; for cross-tenant secrets a
  defence-in-depth deployment should pin the Firecracker / container runtimes.
