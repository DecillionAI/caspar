# Getting Started 🚀

This guide reflects the current `node/` runtime wiring and scripts.

## 1) Prerequisites

- **Go 1.24.x** (see `node/go.mod`)
- **Rust + Cargo** (for `node/appengine`)
- **Docker** (required for docker runtime flows)
- **TLS cert/key files** readable by the node process
- **Firebase service account JSON**
- **QuestDB** (storage/log features use PostgreSQL wire)

Optional runtime helpers:

- Firecracker tooling scripts (`node/scripts/install-fcvmm.sh`, `node/scripts/run-fcvmm.sh`)
- gVisor tooling script (`node/scripts/install-gvisor.sh`)
- ImageMagick (`convert`) for entity image transform helpers

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
| `IPADDR` | chain advertise address |
| `ROOT_NODE` | bootstrap/root origin for free-node logic |
| `IS_HEAD` | head-node mode toggle |
| `VM_EXEC_COST_PER_SECOND` | optional execution pricing value |

Legacy value still present in template:

- `AdminPassword`

## 3) Runtime Files Expected by Default

Current code/scripts expect these paths unless you patch config handling:

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

### Option A: run the binary directly

```bash
cd node
./kasper
```

When running direct mode, also start dependencies used by storage/runtime paths (for example QuestDB and appengine).

What starts from the node process:

- pprof server on `0.0.0.0:9999`
- TLS TCP/WS client servers
- federation + chain listeners
- HTTPS entity + stream gateways

### Option B: scripted multi-node/testnet flow

Common scripts:

- `node/scripts/prepare-testnet.sh`
- `node/scripts/build-conf.sh`
- `node/scripts/run-testnet.sh`
- `node/scripts/stop-testnet.sh`

## 6) Common Ports (from `sample.env` + scripts)

- `CLIENT_WS_API_PORT` (example deployments often use `8076`)
- `CLIENT_TCP_API_PORT` (example deployments often use `8077`)
- `FEDERATION_API_PORT` (example deployments often use `8078`)
- `BLOCKCHAIN_API_PORT` (example deployments often use `1337`)
- `ENTITY_API_PORT` (example deployments often use `3000`)
- `VM_API_PORT` (example deployments often use `3001`)
- pprof fixed at `9999`

## 7) Smoke Checks

- Process boots without panic
- PEM decode + PKCS8 parse succeed
- `/auths/getServerPublicKey` reachable via action protocol
- chain service endpoints (e.g. `/stats`, `/peers`) respond
- entity API accepts signed requests

## 8) Troubleshooting

- `failed to decode PEM block` / key parse panic:
  - invalid `OWNER_PRIVATE_KEY` formatting or non-PKCS8 key
- TLS startup failures:
  - missing/unreadable cert files
- startup crash around Firebase:
  - invalid/missing `/app/serviceAccounts.json`
- storage/log errors:
  - QuestDB/PG-wire endpoint unavailable
