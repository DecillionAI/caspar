# Caspar Node — Go → Rust Translation: Remaining Work Plan

> **Purpose.** This document is the complete, authoritative plan for finishing
> the translation of the Caspar (`kasper`) Go node to Rust. It records every
> remaining file, the established conventions you **must** follow, the exact
> phase ordering, per-module gotchas, the crates to add, and the
> testing/verification strategy. Follow it top to bottom and the result will
> compile, pass tests, and preserve every feature of the Go original.

---

## 0. Current state

- **Branch:** `claude/epic-hypatia-E9EMH` (all work commits here).
- **19 commits landed, every commit builds green and its tests pass.**
- **59 Rust tests passing** (20 of them hashgraph consensus tests verified
  against Babble's own DAG vectors).
- Original Go sources still live beside the new Rust under `node/src/**`
  (`.go` and `.rs` coexist during migration; Cargo only compiles `.rs`).
- The appengine moved to `node/crates/appengine` (its own nested workspace).

### Already translated (do NOT redo)

| Go source | Rust target | Notes |
|---|---|---|
| `node/appengine/**` | `node/crates/appengine/**` | moved; nested workspace |
| `src/abstract/**` | `src/models/**` | renamed (`abstract` is reserved); `trx/` is now `transaction/` |
| `src/core/module/logger` | `src/core/module/logger.rs` | |
| `chain/common`, `chain/crypto`(+`keys`), `chain/peers` | same paths under `src/drivers/network/chain/` | + tests |
| `chain/hashgraph/**` (all 21 prod files) | `chain/hashgraph/*.rs` | consensus engine; **badger_store.go → `rocks_store.rs`** |
| `chain/config`, `chain/proxy` | same | |
| `chain/node/**` (16 prod files) | `chain/node/*.rs` | Core, Node gossip loop, node_rpc, graph |
| `chain/net`: commands, rpc, transport, inmem_transport | `chain/net/*.rs` | interface + in-memory transport |

### Ported test files (Rust `#[cfg(test)]` modules)

`packet_test`, `lru_test`, `median_test`, `rolling_index_test`, `keys_test`,
`json_peer_set_test`, `event_test`, `block_test`, `caches_test`,
`inmem_store_test`, `badger_store_test`, and **most of `hashgraph_test.go`**
(predicates + full consensus pipeline + GetFrame/Reset/Bootstrap).

### Scope still open

~24K LOC of Go remain (chain transports, the rest of the chain assembly,
`core/module/{core,globe,actor,pool}`, all non-chain drivers, the shell API,
`main`, tooling, and remaining test files). Plus `cmd/casparctl` (separate Go
module, 1 file, ~1088 LOC).

---

## 1. Non-negotiable conventions (established across 19 commits)

Deviating from these will break compilation or wire/hash compatibility.

### 1.1 Structure
- **One Go package → one Rust module directory** with `mod.rs`.
  **One Go file → one Rust file.** `mod.rs` re-exports every public item so
  `pkg::Item` resolves exactly like the Go `pkg.Item`.
- `abstract` is a reserved word → the module is `abstractions`.
- Crate root (`src/main.rs`) keeps `#![allow(dead_code, unused_imports)]`
  during migration; declares each top-level module with `mod`.

### 1.2 Type mapping
| Go | Rust |
|---|---|
| `int`, `int64` | `i64` |
| `int32` | `i32` · `uint32` → `u32` · `uint64` → `u64` · `uint8` → `u8` |
| `error` (return) | `anyhow::Result<T>` (`(T,error)` → `Result<T>`; `error` → `Result<()>`) |
| `error` (callback arg) | `Option<anyhow::Error>` |
| `interface{}` / `any` (JSON data) | `serde_json::Value` |
| `interface{}` / `any` (dynamic, downcast) | `crate::util::AnyVal` (`Arc<dyn Any+Send+Sync>`) |
| Go interface | `pub trait …: Send + Sync` used as `Arc<dyn Trait>` |
| variadic `...T` | `&[T]` |
| `map[K]V` | `HashMap<K,V>` (or `BTreeMap` — see 1.4) |
| concurrent `cmap.ConcurrentMap` | `dashmap::DashMap` |
| `time.Time` | `chrono::DateTime<Utc>` (or `std::time::SystemTime`/`Instant`) |
| `time.Duration` | `std::time::Duration` |
| `*T` (nullable) | `Option<T>` |
| `*T` (shared, immutable-ish) | `Arc<T>` |
| `*T` (shared, mutable) | `Arc<Mutex<T>>` |
| Go method (PascalCase) | `snake_case` |
| struct field (PascalCase) | `snake_case` + `#[serde(rename=…)]` |

### 1.3 Serialization
- Structs **with** `json:"x"` tags → `#[serde(rename="x")]` per field.
- Structs **without** json tags → `#[serde(rename_all="PascalCase")]`.
- `[]byte` JSON field → `#[serde(with="crate::util::bytes_base64", default)]`.
- `[][]byte` JSON field → `#[serde(with="crate::util::bytes_base64_vec")]`.
- Go `encoding/json` `Encoder.Encode` appends a trailing `\n` — replicate with
  `b.push(b'\n')` when the bytes are hashed (events, blocks, peers, roots …).
- Go `ugorji` **canonical** codec → `serde_json` **+ `BTreeMap`** for every map
  that participates in a hash (`Frame.roots`, `RoundInfo.created_events`, …).
  Determinism is mandatory for consensus.
- Integer-tagged enums (`TransactionType`, `Trilean`) → `serde_repr`.

### 1.4 Concurrency
- Components accessed exclusively (Hashgraph, Core) → `&mut self` methods.
- Shared components → `Arc<Mutex<…>>`; trait objects → `Arc<dyn …>`.
- Go channels → `crossbeam_channel`. `close(ch)` broadcast → drop the sender
  (store it as `Mutex<Option<Sender<…>>>`; `take()` to close).
- Goroutines → `std::thread::spawn` capturing `Arc` clones. `Run`-style
  methods take `self: &Arc<Self>`.
- Lazy caches → `OnceLock<T>` + a **manual `Clone`** via
  `crate::util::clone_once_lock`.

### 1.5 Datastores (per the task brief)
- **Badger → RocksDB.** The hashgraph store is `rocks_store.rs`
  (`rocksdb::DB`). The node's main KV DB (Go `badger.DB` in `IStorage`) →
  `rocksdb::TransactionDB`, a **separate RocksDB instance co-located with the
  appengine's RocksDB**.
- **QuestDB ("questd") through Rust.** Go used `pgx` via `database/sql` to
  QuestDB on `:8812` (pg-wire). Rust: `r2d2` + `r2d2_postgres` pooled
  `postgres` client (`postgres::tls::NoTls`, `sslmode=disable`). Type alias
  `TsDb` already defined in `abstractions/adapters/storage.rs`.

### 1.6 Shims already provided — reuse them
- `crate::golog` — macros `go_println!`, `go_printf!`, `go_fatal!`,
  `go_panic!` for Go's `log` package.
- `crate::logrus` — `Entry`, `Logger`, `Level`; `.with_field`, `.with_error`,
  `.debug/info/warn/error`. Use for any Go `logrus` usage.
- `crate::util` — `bytes_base64`, `bytes_base64_vec`, `clone_once_lock`,
  `AnyVal`, `GoError`.
- `crate::multipart::FileHeader` — Go `mime/multipart.FileHeader`.

### 1.7 Workflow per phase
1. Translate the module's files.
2. `cd node && cargo build -p caspar-node` → **must be clean.**
3. `cargo test -p caspar-node` → **all tests must pass.**
4. Commit: `Phase N: <summary>` ending with the session URL line.
5. Push with exponential-backoff retry (2s/4s/8s/16s):
   `git push origin claude/epic-hypatia-E9EMH`.
6. **Never land a red commit.** A partial module is fine if it compiles
   (declare only translated submodules in `mod.rs`).

---

## 2. Phase 2 — finish the chain stack

Translate under `src/drivers/network/chain/`. Update each package `mod.rs`
and `chain/mod.rs` as modules are added.

### Phase 2.13 — net transports (~1.0K LOC)
Files → Rust: `net/stream_layer.go`, `net/net_transport.go`,
`net/tcp_stream_layer.go`, `net/tcp_transport.go`, `net/webrtc_conn.go`,
`net/webrtc_stream_layer.go`, `net/webrtc_transport.go`.

- **`StreamLayer` trait**: Go embeds `net.Listener` + `Dial`. Model as a trait
  with `accept() -> Result<Box<dyn ReadWrite>>`, `dial(addr, timeout)`,
  `advertise_addr()`, `addr()`, `close()`. Connections impl
  `std::io::Read + Write + Send` (a `ReadWrite` super-trait alias).
- **`net_transport.go`** is the generic `NetworkTransport` over a
  `StreamLayer`: a consumer thread `accept`s connections, decodes an RPC
  command, dispatches it on the consumer channel; `makeRPC` dials, encodes,
  waits. **RPC wire codec:** Go used `encoding/gob`; Rust has no gob — use
  **`bincode`** (add crate) with the existing `serde` derives on
  `commands.rs`. A 1-byte command tag selects the variant. Keep a connection
  pool keyed by target (Go uses `MaxPool`).
- **TCP**: `tcp_stream_layer` over `std::net::TcpListener`/`TcpStream`;
  `tcp_transport` = `NetworkTransport` + TCP stream layer + TLS
  (`rustls` via the `TlsConfig` carrier already in
  `abstractions/adapters/network/network.rs`).
- **WebRTC**: add the **`webrtc`** crate (Rust port of pion). `webrtc_conn`
  wraps a DataChannel as a `Read+Write` conn; `webrtc_stream_layer` does
  offer/answer via the signal client; `webrtc_transport` = `NetworkTransport`
  + WebRTC stream layer. This is the heaviest item — budget accordingly.
- Ports for these tests: `net_transport_test.go`, `rpc_test.go`,
  `tcp_transport_test.go`, `transport_test.go`, `webrtc_stream_layer_test.go`.

### Phase 2.14 — WAMP signalling (~0.4K LOC)
Files: `net/signal/{promise.go,signal.go}`,
`net/signal/wamp/{client.go,server.go,wamp.go}` + `wamp_test.go`.

- **Risk: there is no mature Rust WAMP crate.** Options, in order of
  preference: (a) implement the minimal WAMP subset Babble uses
  (CALL/RESULT/REGISTER/INVOCATION over WebSocket JSON) on top of
  `tokio-tungstenite`; (b) evaluate `wamp_async`. Keep the `signal::Signal`
  trait small so the implementation is swappable.
- `signal/signal.go` defines the `Signal` interface — translate as a trait;
  the WebRTC stream layer depends only on the trait.

### Phase 2.15 — chain service + dummy + mobile (~1.2K LOC)
- `service/service.go` — Babble's HTTP API. Go `gorilla/mux` → **`axum`** (or
  `hyper`). JSON via `serde_json`.
- `dummy/{inmem_dummy,socket_dummy,state}.go` — the dummy app client used by
  tests; `dummy/state.go` is a tiny app state machine. Ports:
  `inmem_dummy_test.go`, `socket_dummy_test.go`.
- `mobile/{handlers,mobile_app,node,utils}.go` — gomobile bindings. Low
  priority; translate as plain Rust (no FFI needed unless mobile builds are
  required). Mark with a `mobile` cargo feature if you want it excluded.

### Phase 2.16 — `babble.go` assembly (~0.36K LOC)
`babble/babble.go` wires Config + Store + Transport + Proxy + Node together
(the `Babble` engine struct, `NewBabble`, key loading, peer-file loading,
`Init`, `Run`). Depends on everything above. Ports: `babble_test.go`,
`example_test.go`.

### Phase 2.17 — `chain.go` — the Caspar `IChain` driver (~0.4K LOC)
`src/drivers/network/chain/chain.go` implements
`abstractions::adapters::network::IChain` on top of the Babble engine. This is
the bridge between the Caspar node and Babble. Translate last in Phase 2.

### Phase 2.18 — remaining hashgraph + node tests
- Rest of `hashgraph_test.go`: `TestFork`, `TestInsertEventsWithBlockSignatures`,
  `TestFunkyHashgraph{Fame,Blocks,Reset}`, `TestSparseHashgraphReset`.
- All of `hashgraph_dyn_test.go` (861 L — dynamic membership).
- Node tests: `core_test.go` (1046 L), `node_test.go` (842 L),
  `node_rpc_test.go`, `node_dyn_test.go`, `node_extra_test.go`,
  `node_fastsync_test.go`, `node_suspend_test.go`. These use the
  `InmemTransport` already translated.

---

## 3. Phase 3 — core modules (~2.3K LOC)

`src/core/module/` (the `logger` submodule is done).

- `pool/pool.go` (27 L) — trivial; do first.
- `core/core.go` (741 L) — implements `abstractions::models::core::ICore`;
  the central node object. Holds `ITools`, `IActor`, `IGlobe`.
- `globe/globe.go` (468 L) — implements `IGlobe` (chain request/response
  plumbing, staking, elections).
- `actor/**` (1022 L) — `actor.go` + `model/{base,secured,state,trx}`:
  the action dispatch system. `model/trx/trx.go` implements
  `abstractions::models::trx::ITrx` over the RocksDB `TransactionDB`
  (see 1.5). `model/secured` implements `ISecureAction`. `model/state`
  implements `IState`.

**Gotcha:** `ITrx` wraps a RocksDB transaction. Use
`rocksdb::TransactionDB::transaction()`; `commit()`/`discard()` map directly.
The `ITrx` trait methods are `&self` — keep the transaction behind a
`RefCell`/`Mutex` inside the concrete type.

---

## 4. Phase 4 — drivers (~4.0K LOC)

`src/drivers/` (excluding `network/chain`, done).

| Go | Rust | Notes / crates |
|---|---|---|
| `file/file.go` | `drivers/file/` | impl `IFile`; `std::fs`, `tar` crate |
| `security/security.go` | `drivers/security/` | impl `ISecurity`; `rsa`, `sha2`, x509 via `rsa`/`x509-parser` |
| `signaler/signaler.go` | `drivers/signaler/` | impl `ISignaler`; `dashmap` |
| `storage/storage.go` | `drivers/storage/` | impl `IStorage`; **RocksDB `TransactionDB` + QuestDB pool** (see 1.5) |
| `vmm/**` (5 files, 1349 L) | `drivers/vmm/` | impl `IVmm`; talks to the appengine over **ZMQ** (`zmq` crate). `hostcall_entities.go` = the host-call protocol |
| `network/client/{tcp,ws}` (830 L) | `drivers/network/client/` | impl `ITcp`/`IWs`; **`tokio` + `tokio-tungstenite`** (ws) and raw `tokio` TCP. Go used `lxzan/gws` |
| `network/federation/**` (3 files, 844 L) | `drivers/network/federation/` | impl `IFederation`; HTTP between orgs — `axum`/`reqwest` |

**Gotcha:** the vmm driver spawns/communicates with the appengine process.
Appengine stays a **separate binary** (`node/crates/appengine`); the vmm driver
spawns it and exchanges messages over ZMQ — do **not** link appengine as a
library.

---

## 5. Phase 5 — the shell API (~3.8K LOC, 83 files)

`src/shell/` — the largest mechanical block.

- `shell/utils/{crypto,future,origin,vaidate}` — helpers first (note the Go
  typo `vaidate`; keep or fix consistently). Go `go-playground/validator` →
  the **`validator`** crate (derive `Validate`).
- `shell/api/model/**` (8 files) — request/response model structs.
- `shell/api/inputs/**` — `IInput` implementations (users 16, program 15,
  auth, creatures, invites, stores). Mechanical.
- `shell/api/outputs/**` — response payload structs.
- `shell/api/actions/**` — `IAction`/`ISecureAction` implementations
  (auth, creature, dummy, program).
- `shell/api/pluggers/**` — `IPlugger` implementations.
- `shell/api/updates/**` — update handlers.
- `shell/api/main/main.go` — `PlugAll` wiring.
- `shell/shell.go` (root) — the `Kasper` app struct: `NewApp`, `Load`,
  `Close`, `Tools()`.

**Gotcha:** these are highly repetitive — translate one of each kind
carefully, then apply the same pattern. Watch the `ExtendedField` plumbing
(`abstractions::models::action::ExtendedField`) used by `PlugAll`.

---

## 6. Phase 6 — top level, tooling, cleanup (~0.7K LOC + cleanup)

- `node/main.go` → `src/main.rs`: `.env` via `dotenvy`, RSA key parse, signal
  handling (`SIGINT`/`SIGTERM`), the pprof HTTP server (skip or use `axum`),
  `app.Load(...)`, `PlugAll(...)`, `network.Run(...)`. Wire every module.
- `node/builder/pluggergen.go` → a `[[bin]]` or build script (codegen tool).
- `node/keygen/keygen.go` → a `[[bin]]` (RSA keypair generator).
- `src/telemetry/server.go` → `src/telemetry/`: an HTTP telemetry server;
  Go used `badger` → RocksDB; `axum` for HTTP.
- `src/bots/sampleBot/**` (76 L) → `src/bots/`.
- **`cmd/casparctl`** (separate Go module, `cmd/casparctl/go.mod`, ~1088 L):
  the CLI. Translate to its own Rust binary crate `cmd/casparctl` (own
  `Cargo.toml`) or a workspace member. Use `clap` for arg parsing.

### Final cleanup checklist
- [ ] Delete every `node/src/**/*.go` once its `.rs` replacement is verified.
- [ ] Delete `node/main.go`, `node/builder/*.go`, `node/keygen/*.go`.
- [ ] Delete `node/go.mod`, `node/go.sum`, `node/makefile` (Go).
- [ ] Update `node/Dockerfile` for the Rust build (`cargo build --release`).
- [ ] Update `node/scripts/*` that reference `go run`/`go build`.
- [ ] Remove `#![allow(dead_code, unused_imports)]` from `main.rs`; fix the
      warnings that surface (or keep targeted `#[allow]`s).
- [ ] `cargo clippy` clean-up pass.
- [ ] Confirm `node/crates/appengine` still builds independently.
- [ ] Translate `cmd/casparctl`.

---

## 7. Crates to add (beyond those already in `Cargo.toml`)

Already present: `serde`, `serde_json`, `serde_repr`, `anyhow`, `thiserror`,
`dashmap`, `log`, `rsa`, `rocksdb`, `r2d2`, `r2d2_postgres`, `postgres`,
`tar`, `base64`, `uuid`, `chrono`, `k256`, `num-bigint`, `sha2`, `hex`,
`rand`, `getrandom`, `crossbeam-channel`.

Add as needed:

| Crate | For |
|---|---|
| `bincode` | RPC wire codec in `net_transport` |
| `rustls` + `rustls-pemfile` | TLS for TCP/WS/federation/service |
| `tokio` (`rt-multi-thread,macros,net,io-util,time`) | async network drivers |
| `tokio-tungstenite`, `tungstenite` | WebSocket (client `ws`, WAMP signalling) |
| `webrtc` | WebRTC transport (pion port) |
| `axum` or `hyper` | HTTP: chain `service`, `federation`, `telemetry` |
| `reqwest` | outbound HTTP (federation, firebase REST) |
| `zmq` | vmm ↔ appengine IPC |
| `validator` | `shell/utils/vaidate` (go-playground/validator) |
| `dotenvy` | `.env` loading in `main` |
| `clap` | `cmd/casparctl` |
| `x509-parser` / `der` | x509 cert parsing (`security`, TLS) |

**Firebase** (`firebase.google.com/go`): no official Rust SDK — implement the
specific calls used (likely FCM push / token verify) over `reqwest` against
the REST/JWT endpoints. Audit `security`/`shell` for the exact usage first.

**Network policy note:** git deps (`webrtc`, etc.) and crates.io must be
reachable; if the environment blocks them, vendor the crate or stub the
transport behind a cargo feature.

---

## 8. Testing & verification strategy

1. **Per phase:** `cargo build` + `cargo test` green before committing.
2. **Port every `*_test.go`** alongside its module as a `#[cfg(test)] mod
   tests` (in the same file when it must reach private items — as done for
   `hashgraph.rs`). Keep the Go test name in a comment
   (`// Translation of X_test.go::TestY`).
3. **Consensus parity (highest priority):** the hashgraph already has 20
   ported tests asserting bit-identical rounds/fame/blocks. Keep that bar for
   `node` tests — they assert end-to-end consensus over `InmemTransport`.
4. **DB round-trips:** RocksDB `rocks_store` and the new KV/QuestDB storage
   driver each need a write→close→reopen→read test (see
   `test_bootstrap`/`test_db_*` for the pattern; use unique temp dirs).
5. **Determinism:** any change to a hashed structure must keep `serde_json` +
   `BTreeMap` ordering. Add a marshal-stability test if in doubt.
6. **Integration smoke test:** after Phase 6, run the node from `main.rs`
   against a local QuestDB + appengine and exercise the TCP/WS client APIs.
7. `cargo test --workspace` at the end (includes appengine crates if the
   native deps are available).

---

## 9. Risk register

| Risk | Mitigation |
|---|---|
| No mature Rust WAMP crate | Implement the minimal WAMP subset on `tokio-tungstenite` behind the `Signal` trait |
| `webrtc` crate API differs from pion | Keep `webrtc_*` behind the `StreamLayer` trait; isolate API churn |
| Go `encoding/gob` has no Rust equivalent | Use `bincode`; all nodes are Rust so the codec only needs internal consistency |
| Firebase SDK absent | Reimplement the few calls over `reqwest` |
| Native build deps (`rocksdb`, `zmq`, `webrtc`, appengine `wasmedge`) | Ensure system libs/clang/cmake present; gate heavy bits behind cargo features |
| Event/Block hash drift Go↔Rust | Irrelevant if the whole network upgrades to Rust; **keep Rust-internal determinism** (BTreeMap, trailing `\n`, base64) |
| `Core`/`Hashgraph` reentrancy | Already solved: commit callback pushes to a shared queue drained after each run — reuse this pattern anywhere a callback must re-enter its owner |

---

## 10. Phase summary & ordering

```
Phase 2.13  net transports (StreamLayer, net_transport, tcp_*, webrtc_*)   ~1.0K
Phase 2.14  WAMP signalling (signal, signal/wamp)                          ~0.4K
Phase 2.15  chain service + dummy + mobile                                 ~1.2K
Phase 2.16  babble.go (engine assembly)                                    ~0.4K
Phase 2.17  chain.go (Caspar IChain driver)                                ~0.4K
Phase 2.18  remaining hashgraph + all node tests                           ~6.9K (tests)
Phase 3     core/module/{pool,core,globe,actor}                            ~2.3K
Phase 4     drivers/{file,security,signaler,storage,vmm,client,federation} ~4.0K
Phase 5     shell/** (api + utils)                                         ~3.8K
Phase 6     main.rs, builder, keygen, telemetry, bots, cleanup, casparctl  ~1.7K
```

Translate strictly in dependency order: a module's dependencies must already
be translated (or stubbed in `mod.rs`) before it. Within Phase 2 the order
above is mandatory (`chain.go` last). Phases 3→4→5→6 are sequential.

**End state:** `node/` is a pure-Rust Cargo workspace — `node/src` the
translated node binary, `node/crates/appengine` the (separate, Rust) app
engine — with no `.go` files remaining and all ported tests green.
