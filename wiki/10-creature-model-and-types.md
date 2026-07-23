# Creature model & type registry

A **creature** is the general model for *every being* on the Caspar network that
can act — hold a balance, own resources, hold accesses, send signals. Humans are
creatures; the non-human machines that own programs are creatures too. There is
**no separate `User` or `Machine` model** — those are simply creature *types*.

## Base model

Every creature shares one fixed base, stored under `obj::Creature::{id}`:

| field | meaning |
|-------|---------|
| `id` | unique creature id (`{n}@{origin}`) |
| `type` | the creature's registered type (e.g. `human`, `machine`) |
| `username` | unique handle |
| `publicKey` | RSA public key — the identity the signed action protocol verifies against |
| `balance` | token balance (i64) — the single wallet |

`Creature` is the *sole* record and the single source of truth. Identity
(`publicKey`, `type`, `username`), the wallet (`balance`), and program ownership
(`ownerId`, chain/subchain ids) all live on it. Every balance-moving action
(`transfer`, `mint`, `lockToken`, `consumeLock`) reads and writes
`Creature.balance`, and every read (`/creatures/get`, `authenticate`, …) returns
it, so a live fetch is always correct. No mirrored rows, nothing to diverge.

## Extensible type registry

On top of the fixed base, the host managing Caspar registers **customized
creature types**. A type is just a named spec:

```json
{ "initialBalance": 1000000000000000, "customFields": [], "desc": "…" }
```

| spec field | effect |
|------------|--------|
| `initialBalance` | balance seeded when a creature of this type is created |
| `customFields` | additional declared fields `[{name, type, required, default}]` on top of the base |
| `desc` | human description |

Behaviour is driven by the creature's `type` field and its spec — there are no
ad-hoc boolean flags. Types are stored under `Json::CreatureType::{name}` and
enumerated through the `CreatureTypeExists::{name}` link index.
`/creatures/create` looks up the requested type to seed the initial balance
(with a built-in fallback for `human`/`machine` so the very first creature,
created before install runs, still works); unknown types are rejected.

### Built-in types

Registered idempotently in the shell API's **install (bootstrap) phase**
(`install_creature_types`, called from a namespace's `install()`):

- **`human`** — the primary being. `initialBalance = 1e15`.
- **`machine`** — a non-human being that owns programs. `initialBalance = 0`.
  ("Machines" in the program/deploy API are exactly the creatures of type
  `machine`; `/machines/list` filters creatures by `type == "machine"`.)

Registration is *register-if-absent*, so it is safe to run from every
namespace's install and to add further host-defined types the same way.

### Inspecting the registry

```bash
# POST /creatures/types
# -> { "types": [ { "name": "human", ... }, { "name": "machine", ... } ] }
```
