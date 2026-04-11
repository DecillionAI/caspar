# Getting Started 🚀

> Updated: **2026-04-10**

This guide reflects the current `node/` runtime wiring and scripts.

## 1) Prerequisites

- **Go 1.24.x** (see `node/go.mod`)
- **Rust + Cargo** (for `node/appengine`)
- **Docker** (required for docker runtime flows)
- **TLS cert/key files** readable by the node process
- **Firebase service account JSON**
- **QuestDB** (storage/log features use PostgreSQL wire)

Optional helpers:

- Firecracker scripts: `node/scripts/install-fcvmm.sh`, `node/scripts/run-fcvmm.sh`
- gVisor installer: `node/scripts/install-gvisor.sh`
- ImageMagick (`convert`) for entity image transformations

## 2) Environment Setup

```bash
cd node
cp sample.env .env
```

Fill at least these variables:

| Variable | Purpose |
|---|---|
| `OWNER_ID` | owner identity used for boot |
| `OWNER_PRIVATE_KEY` | owner PEM private key (PKCS8 parse path) |
| `ORIGIN` | origin/domain identifier |
| `STORAGE_ROOT_PATH` | root file storage path |
| `BASE_DB_PATH` | base badger path |
| `APPLET_DB_PATH` | applet badger path |
| `SEARCH_INDEX_PATH` | search index path |
| `STORE_LOGS_DB` | store logs DB selector/path |
| `CLIENT_WS_API_PORT` | TLS WS client API port |
| `CLIENT_TCP_API_PORT` | TLS TCP client API port |
| `FEDERATION_API_PORT` | federation TCP port |
| `BLOCKCHAIN_API_PORT` | chain network port |
| `ENTITY_API_PORT` | HTTPS entity API port |
| `VM_API_PORT` | HTTPS VM stream API port |
| `TELEMETRY_API_PORT` | telemetry HTTP API port (default `9099`) |
| `TELEMETRY_DB_PATH` | optional Badger path for cached telemetry snapshots |
| `IPADDR` | chain advertise address |
| `ROOT_NODE` | bootstrap/root origin for free-node logic |
| `IS_HEAD` | head-node mode toggle |
| `VM_EXEC_COST_PER_SECOND` | optional execution pricing |

Legacy value still present in template:

- `AdminPassword`

## 3) Runtime Files (Default Paths)

- Certs:
  - `/app/certs/fullchain.pem`
  - `/app/certs/privkey.pem`
- Firebase:
  - `/app/serviceAccounts.json`

## 4) Build

```bash
cd node/appengine
cargo build

cd ../
CGO_ENABLED=1 go build -o kasper .
```

## 5) Run

### Option A: Full CLI-managed Docker flow (recommended)

```bash
cd cmd/casparctl
go install .

cd ../../node
# one-command install + dependency encapsulation in container
casparctl install --name caspar-node

# realtime TUI dashboard
casparctl stats
```

When the repository layout is standard (`cmd/` and `node/` as siblings), `casparctl` auto-detects the node project directory. The chosen install name is saved to `.casparctl-name` in the project folder and reused by all control/dashboard commands.

Lifecycle control commands:

```bash
casparctl start
casparctl pause
casparctl resume
casparctl stop
casparctl uninstall
casparctl purge
```

### Option B: Direct binary

```bash
cd node
./kasper
```

When running directly, also start dependencies used by storage/runtime paths (for example QuestDB and appengine).

Node process starts:

- pprof server on `0.0.0.0:9999`
- telemetry server on `0.0.0.0:${TELEMETRY_API_PORT:-9099}` (`/telemetry/snapshot`)
- TLS TCP/WS client servers
- federation + chain listeners
- HTTPS entity + stream gateways

### Option C: Scripted multi-node/testnet flow

- `node/scripts/prepare-testnet.sh`
- `node/scripts/build-conf.sh`
- `node/scripts/run-testnet.sh`
- `node/scripts/stop-testnet.sh`

## 6) Common Ports

- `CLIENT_WS_API_PORT` (common example: `8076`)
- `CLIENT_TCP_API_PORT` (common example: `8077`)
- `FEDERATION_API_PORT` (common example: `8078`)
- `BLOCKCHAIN_API_PORT` (common example: `1337`)
- `ENTITY_API_PORT` (common example: `3000`)
- `VM_API_PORT` (common example: `3001`)
- pprof fixed at `9999`

## 7) Smoke Checks ✅

- process boots without panic
- PEM decode + PKCS8 parse succeed
- `/auths/getServerPublicKey` reachable via action protocol
- chain service endpoints (e.g. `/stats`, `/peers`) respond
- entity API accepts signed requests

## 8) Troubleshooting 🛠️

- `failed to decode PEM block` / key parse panic
  - invalid `OWNER_PRIVATE_KEY` formatting or non-PKCS8 key
- TLS startup failures
  - missing/unreadable cert files
- startup crash around Firebase
  - invalid/missing `/app/serviceAccounts.json`
- storage/log errors
  - QuestDB/PG-wire endpoint unavailable
