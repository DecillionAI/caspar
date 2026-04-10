# Caspar Protocol 🌐

Caspar is a decentralized protocol stack built around:

- a customized **Babble/Hashgraph** consensus layer
- **federation-first** cross-origin interoperability
- multi-runtime compute (`wasm`, `docker`, `javascript`, `elpify`, `elpian`)

This repository contains the primary node implementation in `node/`.

## Why Caspar ✨

- **Hashgraph-backed ordering** for distributed request consistency.
- **Federated execution model** for local + remote-origin actions.
- **Signed request protocol** over TLS TCP/WS.
- **Real-time signaling** for users and store groups.
- **Programmable runtime layer** for machine/program deployment.
- **Entity + stream gateways** for user/store/app binary workflows.

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

- **Action protocol**: TLS `tcp` + `ws` binary packet interface.
- **Federation transport**: TLS TCP origin-to-origin messaging.
- **Hashgraph service API**: chain stats/blocks/graph/peers endpoints.
- **HTTPS gateways**:
  - entity API (`ENTITY_API_PORT`)
  - VM stream API (`VM_API_PORT`)

For complete protocol details, see `docs/API_REFERENCE.md`.

## Quick Start 🚀

```bash
cd node
cp sample.env .env
```

Then configure `.env` and required runtime files:

- certs at `/app/certs/fullchain.pem` and `/app/certs/privkey.pem`
- Firebase credentials at `/app/serviceAccounts.json`

Build + run (direct mode):

```bash
cd node/appengine && cargo build
cd ../ && CGO_ENABLED=1 go build -o kasper .
./kasper
```

Direct mode typically requires your supporting services to be available (for example QuestDB and appengine).

## Route Naming Note ⚠️

Current API naming is historical and intentionally preserved:

- `/machines/*` routes manage **Machine** resources.
- `/programs/*` routes manage **Program** resources attached to machines.

Use `docs/API_REFERENCE.md` as the source-of-truth route catalog.

## Main Feature Domains

- **Users**: login/auth, metadata, mint/transfer, lock/consume token.
- **Stores**: create/join/leave, membership/access, signaling, history.
- **Invites**: create/cancel/accept/decline + list endpoints.
- **Machines/Programs**: create/update/delete, deploy, run/stop, logs.
- **Storage/Entities**: uploads/downloads for user/store/app scope.
- **Chain Ops**: create shard/work chains, register nodes, submit base trx.
- **PC Tools**: command execution endpoints.

## Documentation

- `docs/GETTING_STARTED.md` — prerequisites, env vars, build/run, troubleshooting.
- `docs/API_REFERENCE.md` — packet protocol, statuses, routes, APIs.
- `docs/ARCHITECTURE.md` — core internals, chain/federation/runtime/storage layers.
