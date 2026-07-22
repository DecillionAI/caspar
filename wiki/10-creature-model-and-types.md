# Creature model & type registry

A **creature** is the general model for every being on the Caspar network that
can *act* — hold a balance, own resources, hold accesses, send signals. Humans
are creatures; the non-human machines that own programs are creatures too.

## Base model

Every creature shares a fixed base, stored under `obj::Creature::{id}`:

| field | meaning |
|-------|---------|
| `id` | unique creature id (`{n}@{origin}`) |
| `username` | unique handle |
| `publicKey` | RSA public key — the identity the signed action protocol verifies against |
| `balance` | token balance (i64) — the **single** wallet authority |

`Creature` is the sole authoritative record. Balance-moving actions
(`/creatures/transfer`, `/creatures/mint`, `/creatures/lockToken`,
`/creatures/consumeLock`) all read and write `Creature.balance`, and every read
(`/creatures/get`, `authenticate`, …) returns it — so a live fetch is always
correct.

> A legacy `obj::User::{id}` mirror is still written for a few identity-only
> readers (username search, the wire identity DTO). Its `balance` column is
> vestigial and never read, so it cannot diverge. Identity lookups
> (public key, type, username) read `Creature` first and fall back to the
> mirror only for pre-unification data.

## Extensible type registry

On top of the fixed base, the host managing Caspar registers **customized
creature types**. A type declares its behaviour and any custom fields:

```json
{
  "isHuman": true,
  "ownsPrograms": false,
  "initialBalance": 1000000000000000,
  "customFields": [],
  "desc": "The primary human being on the network."
}
```

| type field | effect |
|------------|--------|
| `initialBalance` | balance seeded when a creature of this type is created |
| `ownsPrograms` | if true, the creature also becomes a Machine (can own programs) |
| `isHuman` | marks the primary human being |
| `customFields` | additional declared fields `[{name, type, required, default}]` |

Types are stored under `Json::CreatureType::{name}` and enumerated through the
`CreatureTypeExists::{name}` link index. `/creatures/create` resolves the
requested type from the registry to decide the initial balance and program
ownership (with a built-in fallback for `human`/`machine` so the very first
creature, created before install runs, still works); unknown types are rejected.

### Built-in types

Registered idempotently in the shell API's **install (bootstrap) phase**
(`install_creature_types`, called from a namespace's `install()`):

- **`human`** — the primary being. `initialBalance = 1e15`, does not own programs.
- **`machine`** — a non-human being that can own programs. `initialBalance = 0`,
  `ownsPrograms = true`.

Registration is *register-if-absent*, so it is safe to run from every
namespace's install and to add further host-defined types the same way.

### Inspecting the registry

```bash
caspar-client --batch "creatures.types"   # or POST /creatures/types
# -> { "types": [ { "name": "human", ... }, { "name": "machine", ... } ] }
```
