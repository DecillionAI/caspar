# Architecture 🧠

## 1) System Goals

- **Decentralized ordering/consensus** with a customized Babble/Hashgraph stack.
- **Federation-first interoperability** across origins.
- **Programmable execution** across multiple runtimes (`wasm`, `docker`, and related toolchains).
- **Signed action pipeline** with guard-based authorization checks.

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

1. protocol handler parses packet and resolves action path
2. security checks validate user/signature/context
3. transaction opens and mutates state
4. local/federated updates are emitted
5. optional chain callback reconciliation finalizes distributed result

## 4) Action and Plugger Layer

Actions are grouped under:

- `node/src/shell/api/actions/*`

Pluggers wire those actions into core:

- `node/src/shell/api/pluggers/*`
- `node/src/shell/api/main/api.go` (`PlugAll`)

This is the source of truth for what endpoints exist at runtime.

## 5) Consensus Layer (`node/src/drivers/network/chain`)

The chain stack provides:

- hashgraph event DAG
- block projection
- peer/gossip transport
- service endpoints (`/stats`, `/graph`, `/peers`, etc.)
- transaction routing by type (`baseRequest`, `appRequest`, `response`, `message`, `election`)

Work-chain operations exposed by actions currently include create, create-shard, create-from-point, register-node, and submit-base-trx.

## 6) Federation Layer (`node/src/drivers/network/federation`)

Federation handles cross-origin:

- action requests
- action responses
- async updates/signals

Key behaviors:

- remote origin resolution and known-peer checks
- callback timeout handling
- local state materialization for remote updates when required

## 7) Runtime/Execution Layer

### Wasm

- machine assignment and execution hooks
- signal and chain execution integration

### Docker

- build/deploy/run/stop/exec support
- machine endpoint proxy integration
- build logs + runtime signaling integration

## 8) State and Storage

### KV state

- Badger-backed object/link/index model
- transaction abstraction in `abstract/models/trx`

### Logs/time-series

- QuestDB via PostgreSQL wire usage patterns

### Entity storage

- user/point/app entity upload/download
- stream relay endpoints for larger payload flow

## 9) Networking Interfaces

- TLS TCP action server
- TLS WS action server
- federation TCP transport
- chain gossip + service HTTP
- HTTPS entity + VM stream gateways

## 10) Security Model

- signature verification against stored public keys
- route-specific guard checks (identity, membership, access policy)
- privileged/god command path embedded in point signal handling (`/points/signal` command packets)

## 11) Boot Composition

At startup, app wiring loads adapters/tools, then installs all pluggers/actions, then starts:

- pprof (`0.0.0.0:9999`)
- TCP/WS/Federation/Chain listeners
- API and signaling loops

For exact startup sequence and env usage, use:

- `node/main.go`
- `node/src/shell/kasper.go`
