# API Reference 📡

> Updated: **2026-05-29**

Caspar exposes:

1. Signed binary action protocol (mutual-TLS TCP + TLS WS)
2. Entity / stream endpoints (`ENTITY_API_PORT`, `VM_API_PORT`)
3. Hashgraph service HTTP endpoints (`BLOCKCHAIN_API_PORT`)
4. Telemetry snapshot HTTP endpoint (`TELEMETRY_API_PORT`)

## 1) Binary Action Protocol

### Framing

```text
[4 bytes body_len (big-endian)]
[body]
```

`body` layout:

```text
[4 bytes signature_len][signature]
[4 bytes user_id_len][user_id]
[4 bytes path_len][path]
[4 bytes request_id_len][request_id]
[payload_json]
```

### Frame tag bytes

A single leading byte tags each frame:

- `0x01` — asynchronous update / signal frame
- `0x02` — synchronous response frame
- `0x03` — request frame

### Status codes

- `0` success
- `1` action not found
- `2` parse/validation error
- `3` execution error
- `4` auth/authorization failure

### Consensus routing

Whether an action is ordered through the Babble chain is determined by its
input's `origin` field, **not** by the route name:

- `origin == "global"` → consensus-bound (e.g. `creatures.create`,
  `creatures.createMachine`, `programs.create`, `programs.deploy`,
  `chains/*`). Produces `Adding Transaction → Commit block=N`.
- `origin == ""` → local (reads, `creatures.signal`, dev `login`). No chain
  activity. See [`CONSENSUS_NOTES.md`](CONSENSUS_NOTES.md).

## 2) Route Groups

### Auth
- `GET /auths/getServerPublicKey`
- `GET /auths/getServersMap`

### Users
- `POST /users/authenticate`, `/users/transfer`, `/users/mint`, `/users/checkSign`
- `POST /users/lockToken`, `/users/consumeLock`, `/users/login`, `/users/create`, `/users/delete`, `/users/update`
- `GET /users/meta`, `/users/get`, `/users/getByUsername`, `/users/find`, `/users/list`

### Stores
- `POST /stores/addMachine`, `/stores/listMachines`, `/stores/updateProgram`, `/stores/removeMachine`
- `POST /stores/addProgram`, `/stores/removeProgram`, `/stores/addMember`, `/stores/updateMember`
- `POST /stores/updateMemberAccess`, `/stores/updateProgramAccess`, `/stores/getDefaultAccess`, `/stores/readMembers`, `/stores/removeMember`
- `POST /stores/create`, `/stores/join`, `/stores/leave`, `/stores/signal`, `/stores/history`
- `PUT /stores/update`, `DELETE /stores/delete`
- `GET /stores/meta`, `/stores/get`, `/stores/read`, `/stores/list`

### Invites
- `POST /invites/create`, `/invites/listStoreInvites`, `/invites/listUserInvites`, `/invites/cancel`, `/invites/accept`, `/invites/decline`

### Machines + Programs
- Machines: `/machines/create`, `/machines/delete`, `/machines/update`, `/machines/myCreated`, `/machines/signal`, `/machines/runProgramEntity`, `/machines/stopProgramEntity`, `/machines/readBuildLogs`, `/machines/readMachineBuilds`, `/machines/deploy`, `/machines/list`, `/machines/listProgramMachines`
- Programs: `/programs/create`, `/programs/delete`, `/programs/list`

### Storage
- `POST /storage/upload`, `/storage/uploadUserEntity`, `/storage/deleteUserEntity`, `/storage/uploadStoreEntity`, `/storage/uploadAppEntity`, `/storage/deleteStoreEntity`, `/storage/download`

### Chains
- `POST /chains/create`, `/chains/createShard`, `/chains/createFromStore`, `/chains/submitBaseTrx`, `/chains/registerNode`

### PC + Misc
- `POST /pc/runPc`, `/pc/execCommand`
- `GET /api/hello`, `/api/time`, `/api/ping`

## 3) HTTPS APIs

> **Note.** In the current build, binary blob I/O (user/store/app entities) is
> exercised through the `storage/*` actions on the binary action protocol above.
> `ENTITY_API_PORT` is reserved/configured for a dedicated entity endpoint; the
> routes below describe that intended surface.

### Entity API (`ENTITY_API_PORT`)
- `/storage/downloadUserEntity`
- `/storage/uploadUserEntity`
- `/storage/uploadStoreEntity`
- `/storage/uploadAppEntity`
- `/storage/downloadAppEntity`
- `/storage/downloadStoreEntity`
- `/stream/get`
- `/stream/send`

### VM stream API (`VM_API_PORT`)
- `/stream/send`

### Telemetry API (`TELEMETRY_API_PORT`, default `9099`) 📈
- `GET /telemetry/snapshot`
