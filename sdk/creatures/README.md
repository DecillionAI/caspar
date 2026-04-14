# VM Creatures (WASM Go Projects) 🧬

> Updated: **2026-04-14**

`creatures` are deployable wasm-go projects that mirror action namespaces and execute through host operations.

## Namespaces

- `stores`
- `invites`
- `storage`
- `pc`
- `chain`

## Endpoint folders

- `endpoints/stores/*`
- `endpoints/invites/*`
- `endpoints/storage/*`
- `endpoints/pc/*`
- `endpoints/chains/*`

## Execution model

1. Read `/input.json` (`path`, `payload`, `userId`, `storeId`)
2. Validate route against namespace allow-list
3. Call host `execShellAction`
4. Return result/update packets

## Recent updates 📌

- Creatures are now organized under SDK and aligned with active shell route namespaces.
- Endpoint modules have been kept in sync with host-op naming updates and runtime behavior.
