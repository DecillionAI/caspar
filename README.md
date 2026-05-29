# Caspar Protocol 🌐

> **Status:** Documentation refreshed on **2026-05-29** to match the current
> Rust codebase (`caspar-node` v0.1.0) and the `reports/final` benchmark run.

**Caspar** is a decentralised protocol stack that unifies hashgraph-style
Byzantine-fault-tolerant (BFT) consensus, federation-native messaging, and a
multi-runtime virtual-machine execution engine under a single **creature**
programming model. Nodes expose a signed binary action protocol over
mutual-TLS TCP/WebSocket transports, execute user-defined WebAssembly
creatures, replicate state through an embedded Babble hashgraph chain, and can
spawn subordinate VMs in any of **six runtimes**: `wasm`, `docker`, `elpify`,
`elpian`, `javascript`, and `firecracker`.

The entire node, the local crates, and the `casparctl` operator CLI are
written in **Rust** (the previous Go implementation is retained only under
`node.old/`).

## ✨ Highlights

- **Hashgraph consensus** — leaderless asynchronous BFT via an embedded Babble
  (Rust) chain; consensus-bound writes are ordered through the chain, reads and
  signals bypass it.
- **Creature programming model** — application logic lives in isolated WASM
  modules (WasmEdge) with a per-creature RocksDB key-value namespace and a
  signal bus.
- **Unified multi-runtime VM router** — `build`/`run`/`exec`/`copy`/`terminate`
  operations dispatch to six runtimes through one runtime-agnostic code path.
- **Per-VM persistent transaction** — all key-value mutations within a single
  creature signal commit atomically as one RocksDB write (no intra-signal write
  amplification, no partially-visible state).
- **elpify-chain** — a commit-reveal Proof-of-Stake validator election that runs
  inside WASM and is attested by Miden **STARK** zero-knowledge proofs.
- **Federation bridge** — authenticated cross-origin request/update propagation
  for composing creatures across independent deployments.
- **Shard-parallel scale-out** — each shard is a self-contained three-node
  hashgraph group; aggregate throughput scales linearly with shard count.
- **Telemetry** — a snapshot HTTP API and a live `casparctl stats` TUI.

## 📊 Current Measured State (`reports/final`, 2026-05-29)

Single shard = three local nodes (8074 / 8174 / 8274) sharing one Babble group.

| Metric | Value |
|--------|-------|
| Workflow correctness | **154 / 154** steps (9 suites, 100%) |
| Peak sequential throughput | **10.5 ops/s** (`chain:submitBaseTrx`) |
| Median consensus round | **≈95 ms** (84 ms floor) |
| Peak STARK throughput | **11.3 proofs/s** @ concurrency 2 |
| Concurrent load success | **100%** across C = 1…32 |
| WASM payload per node | **37 creatures, ≈11.6 MB** |

See [`docs/BENCHMARKS.md`](docs/BENCHMARKS.md) for the full breakdown.
`reports/final/` is the authoritative benchmark artifact.

## 🧭 Repository Map

- `node/` — main node runtime (**Rust**; binaries `caspar-node`, `caspar-keygen`)
  - `node/src/` — action router, core transactions, chain, VM manager, telemetry
  - `node/crates/` — `elpify-lang` (Miden STARK), `elpian-vm`, `async-wasi`,
    `wasmedge-sys`/`-types`/`-macro`
- `cmd/casparctl/` — operator CLI (**Rust**): install / control / telemetry TUI
- `sdk/` — Python client (`caspar_client.py`) + sample creatures
- `docs/` — architecture, API, consensus notes, benchmarks, setup
- `reports/` — benchmark run artifacts (`reports/final/` is current)
- `bench-all.sh`, `run-nodes.sh`, `stop-nodes.sh`, `build-dist.sh` — operations
- `node.old/` — legacy Go implementation (reference only)

## 🛠️ `casparctl` CLI

```bash
# build & install the Rust CLI
make -C node casparctl-install      # or: cargo install --path cmd/casparctl

casparctl install --name caspar-node
casparctl start
casparctl stats        # live telemetry TUI
casparctl pause
casparctl resume
casparctl stop
casparctl uninstall
casparctl purge
```

`casparctl stats` polls `TELEMETRY_API_PORT` (default `9099`) and renders live
throughput, latency percentiles, and consensus round counters. No agent runs on
the node — telemetry is served by the node's own HTTP listener.

## 🚀 Quick Start

```bash
# 1. configure
cd node && cp sample.env .env      # fill in OWNER_ID, ports, paths

# 2. build (Rust; no Go toolchain required)
make build                          # -> target/release/caspar-node

# 3. run a local cluster
cd .. && ./run-nodes.sh             # 3-node shard + QuestDB
```

Full instructions: [`docs/GETTING_STARTED.md`](docs/GETTING_STARTED.md).

## 📚 Documentation

- [`docs/GETTING_STARTED.md`](docs/GETTING_STARTED.md) — prerequisites, build, run
- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — subsystems and mechanisms
- [`docs/API_REFERENCE.md`](docs/API_REFERENCE.md) — wire protocol + routes
- [`docs/CONSENSUS_NOTES.md`](docs/CONSENSUS_NOTES.md) — Babble behaviour & routing
- [`docs/BENCHMARKS.md`](docs/BENCHMARKS.md) — measured performance
- `sdk/README.md` — client SDK and samples
