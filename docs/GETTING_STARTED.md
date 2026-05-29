# Getting Started 🚀

> Updated: **2026-05-29** — matches the current Rust repo layout
> (`node/`, `cmd/casparctl/`, `sdk/`).

The node and the CLI are pure **Rust**; **no Go toolchain is required**.

## 1) Prerequisites

- **Rust** + Cargo (stable, edition 2021)
- A C toolchain (for RocksDB / WasmEdge native builds)
- **QuestDB** (telemetry / time-series over the PostgreSQL wire protocol) —
  `run-nodes.sh` can auto-download the QuestDB jar if Java is present
- **Docker** — only needed for the `docker` / `pc` / Firecracker runtimes
- TLS certificate / key files for the client and federation transports

## 2) Environment Setup

```bash
cd node
cp sample.env .env
```

Key variables (see `node/sample.env` for the full list):

- Identity: `OWNER_ID`, `OWNER_PRIVATE_KEY`, `ORIGIN`
- Client/peer ports: `CLIENT_WS_API_PORT`, `CLIENT_TCP_API_PORT`,
  `FEDERATION_API_PORT`, `BLOCKCHAIN_API_PORT`, `ENTITY_API_PORT`, `VM_API_PORT`
- Telemetry: `TELEMETRY_API_PORT` (default `9099`), `TELEMETRY_DB_PATH`
- Storage paths: `STORAGE_ROOT_PATH`, `BASE_DB_PATH`, `APPLET_DB_PATH`,
  `SEARCH_INDEX_PATH`, `STORE_LOGS_DB`
- Topology: `IPADDR`, `ROOT_NODE`, `IS_HEAD`
- VM cost knobs: `VM_EXEC_COST_PER_SECOND`, `VM_RAM_COST_PER_MB_PER_MINUTE`,
  `VM_CPU_CORE_COST_PER_MINUTE`, `VM_DISK_COST_PER_GB_PER_MINUTE`

## 3) Build

```bash
cd node
make build            # cargo build --release -> target/release/caspar-node
                      # also builds the caspar-keygen binary
```

Other targets: `make test`, `make lint` (clippy), `make doc`,
`make casparctl-install`.

## 4) Run Options

### A) Local cluster (recommended) ✅

```bash
# from the repo root — starts a 3-node shard + QuestDB
./run-nodes.sh
...
./stop-nodes.sh
```

### B) `casparctl`-managed container flow

```bash
make -C node casparctl-install        # or: cargo install --path cmd/casparctl

casparctl install --name caspar-node
casparctl start
casparctl stats                       # live telemetry TUI
casparctl pause | resume | stop | uninstall | purge
```

### C) Direct binary

```bash
cd node
./target/release/caspar-node
```

## 5) Smoke Checks 🧪

- Node starts without panic.
- Telemetry endpoint returns a snapshot: `GET /telemetry/snapshot` (`:9099`).
- Auth key route answers: `/auths/getServerPublicKey`.
- One consensus-bound write (e.g. `chains/submitBaseTrx`) produces a
  `Commit block=N` in the node log (see `docs/CONSENSUS_NOTES.md`).

## 6) Benchmarks

Reproduce the end-to-end workflow + throughput suite:

```bash
./bench-all.sh           # drives all 3 nodes; writes workflow_report.md + JSON
```

Curated runs are archived under `reports/`; the authoritative reference run is
`reports/final/`. See [`BENCHMARKS.md`](BENCHMARKS.md).

## 7) Operational Notes 📌

- Storage moved from Badger to **RocksDB**; telemetry/time-series uses QuestDB.
- VM build/runtime logs are persisted and streamable.
- The Docker / Firecracker / `pc` workflows require a Docker + micro-VM daemon
  and are not exercised in a CPU-only sandbox.
