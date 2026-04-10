# API Reference 📡

This node exposes three interfaces:

1. **Signed binary action protocol** over TLS TCP and TLS WebSocket.
2. **HTTPS entity/stream gateways** for file/entity transfer.
3. **Hashgraph service HTTP API** for network and chain observability.

All route names below are aligned with the current action declarations in `node/src/shell/api/actions/*`.

## 1) Action Protocol (TCP + WS)

### TCP frame format

```text
[4 bytes body_len (big-endian)]
[body]
```

`body` is:

```text
[4 bytes signature_len]
[signature bytes]
[4 bytes user_id_len]
[user_id bytes]
[4 bytes path_len]
[path bytes]               # e.g. /stores/signal
[4 bytes request_id_len]
[request_id bytes]
[payload bytes]            # JSON action input
```

### WS behavior

WS uses the same packet body semantics as TCP request packets, handled by the same action processing pipeline.

### ACK byte

Client ACK frame is one byte:

```text
0x01
```

### Server response frame

```text
0x02
[4 bytes request_id_len]
[request_id]
[4 bytes status_code]
[json response bytes]
```

### Server update frame

```text
0x01
...update payload...
```

## 2) Status Codes

| Code | Meaning |
|---|---|
| `0` | success |
| `1` | action not found |
| `2` | input parse/validation error |
| `3` | action execution error |
| `4` | authentication/authorization failure |

## 3) Authentication Commands

Special command paths handled by the network layer:

- `authenticate`
- `logout`

After successful `authenticate`, queued user signals can be replayed.

## 4) Action Routes (Current)

> Methods/routes listed from current action comments and plugger wiring.

### Auth

- `GET /auths/getServerPublicKey`
- `GET /auths/getServersMap`

### Users

- `POST /users/authenticate`
- `POST /users/transfer`
- `POST /users/mint`
- `POST /users/checkSign`
- `POST /users/lockToken`
- `POST /users/consumeLock`
- `POST /users/login`
- `POST /users/create`
- `POST /users/delete`
- `POST /users/update`
- `GET /users/meta`
- `GET /users/get`
- `GET /users/getByUsername`
- `GET /users/find`
- `GET /users/list`

### Stores

- `POST /stores/addMachine`
- `POST /stores/listMachines`
- `POST /stores/updateProgram`
- `POST /stores/removeMachine`
- `POST /stores/addProgram`
- `POST /stores/removeProgram`
- `POST /stores/addMember`
- `POST /stores/updateMember`
- `POST /stores/updateMemberAccess`
- `POST /stores/updateProgramAccess`
- `POST /stores/getDefaultAccess`
- `POST /stores/readMembers`
- `POST /stores/removeMember`
- `POST /stores/create`
- `PUT /stores/update`
- `DELETE /stores/delete`
- `GET /stores/meta`
- `GET /stores/get`
- `GET /stores/read`
- `POST /stores/join`
- `POST /stores/leave`
- `POST /stores/signal`
- `POST /stores/history`
- `GET /stores/list`

### Invites

- `POST /invites/create`
- `POST /invites/listStoreInvites`
- `POST /invites/listUserInvites`
- `POST /invites/cancel`
- `POST /invites/accept`
- `POST /invites/decline`

### Machines and Programs

> Naming note: in current code, `/machines/*` actions operate on **Machine** models and `/programs/*` actions operate on **Program** models attached to a machine.

- `POST /machines/create`
- `POST /machines/delete`
- `POST /machines/update`
- `GET /machines/myCreated`
- `POST /machines/signal`
- `POST /machines/runProgramEntity`
- `POST /machines/stopProgramEntity`
- `POST /machines/readBuildLogs`
- `POST /machines/readMachineBuilds`
- `POST /machines/deploy`
- `GET /machines/list`
- `GET /machines/listProgramMachines`
- `POST /programs/create`
- `POST /programs/delete`
- `GET /programs/list`

### Storage

- `POST /storage/upload`
- `POST /storage/uploadUserEntity`
- `POST /storage/deleteUserEntity`
- `POST /storage/uploadStoreEntity`
- `POST /storage/uploadAppEntity`
- `POST /storage/deleteStoreEntity`
- `POST /storage/download`

### Chain

- `POST /chains/create`
- `POST /chains/createShard`
- `POST /chains/createFromStore`
- `POST /chains/submitBaseTrx`
- `POST /chains/registerNode`

### PC + Dummy

- `POST /pc/runPc`
- `POST /pc/execCommand`
- `GET /api/hello`
- `GET /api/time`
- `GET /api/ping`

## 5) HTTPS Entity + Stream APIs

Entity server (bound by `ENTITY_API_PORT`) registers:

- `/storage/downloadUserEntity`
- `/storage/uploadUserEntity`
- `/storage/uploadStoreEntity`
- `/storage/uploadAppEntity`
- `/storage/downloadAppEntity`
- `/storage/downloadStoreEntity`
- `/stream/get`
- `/stream/send`

VM gateway (bound by `VM_API_PORT`) registers:

- `/stream/send`

## 6) Hashgraph Service API

Default handlers include:

- `/stats`
- `/block/{index}`
- `/blocks/{start}?count=n`
- `/graph`
- `/peers`
- `/genesispeers`
- `/validators/{index}`
- `/history`

## 7) Input Schema Sources

Authoritative request models are in:

- `node/src/shell/api/inputs/users`
- `node/src/shell/api/inputs/stores`
- `node/src/shell/api/inputs/machine`
- `node/src/shell/api/inputs/invites`
- `node/src/shell/api/inputs/storage`
- `node/src/shell/api/inputs/chain`
- `node/src/shell/api/inputs/auth`
- `node/src/shell/api/inputs/pc`

For machine deploy payloads, use `node/src/shell/api/inputs/machine/deploy.go` as the contract source.
