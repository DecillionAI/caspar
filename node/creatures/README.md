# VM Creatures (WASM Go Projects)

This folder contains separate deployable wasm creature projects for namespaces previously served by shell API directly:

- `points`
- `invites`
- `storage`
- `pc`
- `chain`

Each module is intended to run inside VM and use imported host functions for all host/system effects.

Migration approach:

- Each creature module reads `/input.json` with `{path,payload,userId,pointId}`.
- The module validates the requested endpoint path against its namespace allow-list.
- The module calls host op `execShellAction`, which executes the same secure action parser/algorithm on host.
- This keeps execution in VM creatures while ensuring all mutations happen through host functions.

Per-endpoint wasm projects:

- `endpoints/points/*`
- `endpoints/invites/*`
- `endpoints/storage/*`
- `endpoints/pc/*`
- `endpoints/chains/*`

Each endpoint project is a standalone wasm-go module and reimplements endpoint behavior via micro host functions (`micro*`) such as state/json/link updates, security checks, and signaling operations.
