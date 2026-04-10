# VM Creatures (WASM Go Projects) 🧬

> Updated: **2026-04-10**

This folder contains deployable WASM creature projects for namespaces previously served directly by shell API:

- `stores`
- `invites`
- `storage`
- `pc`
- `chain`

Each module runs inside VM and uses imported host functions for host/system effects.

## Migration Model

- Each creature reads `/input.json` with `{ path, payload, userId, storeId }`.
- The creature validates the requested endpoint path against its namespace allow-list.
- The creature calls host op `execShellAction`, which runs the same secure action parser/algorithm on host.

This keeps execution inside VM creatures while ensuring all mutations flow through secure host functions.

## Endpoint Layout

- `endpoints/stores/*`
- `endpoints/invites/*`
- `endpoints/storage/*`
- `endpoints/pc/*`
- `endpoints/chains/*`

Each endpoint project is a standalone wasm-go module and reimplements endpoint behavior via host functions (state/json/link updates, security checks, signaling, and related operations).
