# Caspar Protocol 🌐

> **Status:** Documentation refreshed on **2026-04-10** to align with current repository layout and runtime wiring.

Caspar is a decentralized protocol stack built around:

- customized **Babble/Hashgraph** consensus
- **federation-first** cross-origin interoperability
- multi-runtime execution (`wasm`, `docker`, `javascript`, `elpify`, `elpian`)

The main node implementation lives in `node/`.

## Why Caspar ✨

- Deterministic request ordering with hashgraph-backed consensus
- Federated execution model for local and remote-origin actions
- Signed request protocol over TLS (`tcp` + `ws`)
- Real-time signaling for users, stores, and runtime workflows
- Programmable runtime layer for machine/program deployment
- Entity + stream gateways for user/store/app binary workflows

## System Snapshot 🧩

```text
Clients (TLS TCP/WS, signed packets)
  -> Action Router + Guards
    -> Core Transaction Layer
      -> Hashgraph Chain
      -> Federation Bridge
      -> Runtime Drivers (Wasm | Docker)
      -> Storage (Badger + QuestDB/PG wire paths)
      -> HTTPS Entity/Stream APIs
```

## Interface Surface 📡

- **Action protocol:** TLS `tcp` + `ws` binary packet interface
- **Federation transport:** TLS TCP origin-to-origin messaging
- **Hashgraph service API:** chain stats/blocks/graph/peers endpoints
- **HTTPS gateways:**
  - entity API (`ENTITY_API_PORT`)
  - VM stream API (`VM_API_PORT`)

For protocol specifics, see `docs/API_REFERENCE.md`.

## Quick Start 🚀

```bash
cd node
cp sample.env .env
```

Configure `.env` and required runtime files:

- certs: `/app/certs/fullchain.pem`, `/app/certs/privkey.pem`
- Firebase credentials: `/app/serviceAccounts.json`

Build + run (direct mode):

```bash
cd node/appengine && cargo build
cd ../ && CGO_ENABLED=1 go build -o kasper .
./kasper
```

Direct mode typically requires supporting services to be available (for example QuestDB and appengine).

## Caspar CLI (`casparctl`) 🛠️

The repository now includes a dedicated CLI for full container lifecycle control and a live terminal dashboard.

Build/install it:

```bash
cd cmd/casparctl
go install .
```

Then use it:

```bash
# One-command install (build image + run container)
casparctl install --name caspar-node

# Lifecycle controls
casparctl pause
casparctl resume
casparctl stop

# Realtime multi-section TUI dashboard (overview, resources, ports, mounts, logs, actions)
casparctl stats

# Cleanup
casparctl uninstall
casparctl purge
```
When `node/` exists near `cmd/`, project-dir is auto-detected. The chosen `--name` is saved to `node/.casparctl-name` and reused by all lifecycle commands (`start`, `pause`, `resume`, `stop`, `uninstall`, `purge`, `stats`).

Caspar node now exposes telemetry APIs on `TELEMETRY_API_PORT` (default `9099`), including `GET /telemetry/snapshot`, and the `casparctl stats` dashboard pulls this endpoint every refresh cycle (default 2 seconds).

## Route Naming Note ⚠️

Current API naming is historical and intentionally preserved:

- `/machines/*` routes manage **Machine** resources
- `/programs/*` routes manage **Program** resources attached to machines

Use `docs/API_REFERENCE.md` as the source-of-truth route catalog.

## Core Feature Domains 🧱

- **Users:** login/auth, metadata, mint/transfer, lock/consume token
- **Stores:** create/join/leave, membership/access, signaling, history
- **Invites:** create/cancel/accept/decline + listing routes
- **Machines/Programs:** create/update/delete, deploy, run/stop, logs
- **Storage/Entities:** uploads/downloads for user/store/app scopes
- **Chain Ops:** create shard/work chains, register nodes, submit base trx
- **PC Tools:** command execution endpoints

## Documentation Map 🗂️

- `docs/GETTING_STARTED.md` — prerequisites, env vars, build/run, troubleshooting
- `docs/API_REFERENCE.md` — packet protocol, statuses, routes, APIs
- `docs/ARCHITECTURE.md` — core internals and subsystem architecture
- `sdk/README.md` — runtime-oriented SDK examples
- `extensions/verifiable-chain/README.md` — verifiable-chain extension toolkit
