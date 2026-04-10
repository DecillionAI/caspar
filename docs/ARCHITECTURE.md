# Architecture 🧠

> Updated: **2026-04-10**

## 1) System Goals

- Decentralized ordering/consensus with customized Babble/Hashgraph
- Federation-first interoperability across origins
- Programmable execution across multiple runtimes (`wasm`, `docker`, and related toolchains)
- Signed action pipeline with guard-based authorization

## 2) High-Level Runtime Graph

```text
TLS TCP/WS Clients
  -> Action Router + Guarded Actions
    -> Core State Transactions
      -> Hashgraph Chain Pipeline
      -> Federation Bridge
      -> Runtime Drivers
      -> Storage + Entity/Stream APIs
```

## 3) Core Layer (`node/src/core/module/core`)

`Core` coordinates:

- action registry and invocation
- guarded transaction lifecycle (`state` + `trx`)
- asynchronous updates/signals
- chain callback reconciliation
- election/validator selection helpers

Typical action flow:

1. Protocol handler parses packet and resolves action path
2. Security checks validate user/signature/context
3. Transaction opens and mutates state
4. Local/federated updates are emitted
5. Optional chain callback reconciliation finalizes distributed result

## 4) Action and Plugger Layer

- Actions: `node/src/shell/api/actions/*`
- Pluggers: `node/src/shell/api/pluggers/*`
- Bootstrap wiring: `node/src/shell/api/main/api.go` (`PlugAll`)

This is the runtime source-of-truth for available actions.

## 5) Consensus Layer (`node/src/drivers/network/chain`)

Provides:

- hashgraph event DAG
- block projection
- peer/gossip transport
- service endpoints (`/stats`, `/graph`, `/peers`, etc.)
- transaction routing by type (`baseRequest`, `appRequest`, `response`, `message`, `election`)

Work-chain actions include create, create-shard, create-from-store, register-node, and submit-base-trx.

## 6) Federation Layer (`node/src/drivers/network/federation`)

Federation handles cross-origin:

- action requests
- action responses
- async updates/signals

Key behaviors:

- remote origin resolution and known-peer checks
- callback timeout handling
- local state materialization for remote updates when required

## 7) Runtime / Execution Layer

### Wasm

- machine assignment and execution hooks
- signal and chain execution integration

### Docker

- build/deploy/run/stop/exec support
- machine endpoint proxy integration
- build logs + runtime signaling integration

## 8) State and Storage

### KV State

- Badger-backed object/link/index model
- transaction abstraction in `abstract/models/trx`

### Logs / Time-Series

- QuestDB via PostgreSQL wire usage patterns

### Entity Storage

- user/store/app entity upload/download
- stream relay endpoints for large payload flow

## 9) Networking Interfaces

- TLS TCP action server
- TLS WS action server
- federation TCP transport
- chain gossip + service HTTP
- HTTPS entity + VM stream gateways

## 10) Security Model 🔒

- signature verification against stored public keys
- route-specific guard checks (identity, membership, access policy)
- privileged command path embedded in `/stores/signal` command packets

## 11) Boot Composition

Startup wiring installs adapters/tools, then pluggers/actions, then starts:

- pprof (`0.0.0.0:9999`)
- TCP/WS/Federation/Chain listeners
- API and signaling loops

For exact startup sequence and env behavior:

- `node/main.go`
- `node/src/shell/kasper.go`
