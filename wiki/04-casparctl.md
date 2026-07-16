# 04 — Casparctl (operator CLI)

`casparctl` is the Rust operator CLI that manages a Caspar node's entire
lifecycle: install/uninstall, start/stop, live telemetry, runtime profiling, VM
plugin selection, and the geo-distributed cluster. It manages the node as a
Dockerized container and shells out to `docker` / `curl` / `openssl`, keeping
the binary small.

**Build & install:**

```bash
make -C node casparctl-install        # or: cargo install --path cmd/casparctl
```

**Global shape:** `casparctl <command> [flags]`. Flags are `--key value` or
`--key=value`. Every command accepts `--help`.

Each command below is documented in its own section.

---

## casparctl install

Full node setup as a Docker container: installs & validates the gVisor runtime
(`runsc`), pulls `nginx:alpine`, creates the node's storage directories,
generates self-signed TLS certs, builds the Caspar image, and bootstraps the
testnet (`prepare-testnet.sh` + `run-testnet.sh`). It saves the chosen names to
`.casparctl-name` / `.casparctl-image` for later commands.

```bash
casparctl install [--project-dir PATH] [--env-file .env] [--envvpath PATH] \
                  [--name kasper] [--container-name node1]
```

- `--project-dir` — path to the node dir (auto-detected via a `Dockerfile` when omitted).
- `--env-file` — env file relative to the project dir (default `.env`).
- `--envvpath` — a ready env file to copy into the project as `--env-file`.
- `--name` — docker image name/tag.
- `--container-name` — container name expected by the testnet run script.

---

## casparctl uninstall

Stop and remove the Caspar container (by the saved name). Idempotent — reports
and exits cleanly if the container does not exist.

```bash
casparctl uninstall [--project-dir PATH]
```

---

## casparctl purge

Uninstall **and** remove the built image plus the `kasper-proxy` helper and the
`kasper:latest` image.

```bash
casparctl purge [--project-dir PATH]
```

---

## casparctl start / pause / resume / stop

Container lifecycle against the saved container name. `resume` maps to
`docker unpause`; `stop` maps to `docker stop`.

```bash
casparctl start   [--project-dir PATH]
casparctl pause   [--project-dir PATH]
casparctl resume  [--project-dir PATH]
casparctl stop    [--project-dir PATH]
```

---

## casparctl run / node-status / node-stop

The container flow above (`install` + `start`) needs a Docker daemon, gVisor,
and the nginx TLS proxy. `casparctl run` is the lightweight alternative for a
sandbox or dev host: it launches the pre-built node binary from `dist/`
directly, after generating a fresh single-node config and starting the QuestDB
instance the node requires — no Docker needed.

```bash
casparctl run [--repo-dir PATH] [--data-dir PATH] [--detach] [--no-questdb]
casparctl node-status [--data-dir PATH]
casparctl node-stop   [--data-dir PATH]
```

- **`run`** — auto-detects the repo (the dir containing `dist/bin/caspar-node`),
  and on first run generates keys (`caspar-keygen`), a PKCS#8 RSA
  `OWNER_PRIVATE_KEY` (openssl), a single-node `.env`, and the babble
  `peers.genesis.json`. It then starts QuestDB (needs Java 11+; uses
  `dist/questdb/questdb.jar`), and launches the node with the bundled WasmEdge
  library on `LD_LIBRARY_PATH` and stock `runc` for container VMs. `--detach`
  leaves the node running after the command returns; `--data-dir` defaults to
  `<repo>/caspar-data/node1`.
- **`node-status`** — shows the node/QuestDB process state, which client ports
  are open, and a telemetry-snapshot liveness probe.
- **`node-stop`** — stops the locally-run node and its QuestDB (by PID file).

The node serves its client transports in **plaintext** here (TLS is normally
terminated by the nginx proxy, which this flow omits). Connect the client CLI
with `CASPAR_TLS=0` (see [Client CLI](09-client-cli.md#connecting-to-a-node)):

```bash
casparctl run --detach
casparctl node-status
CASPAR_TLS=0 CASPAR_PROTO=ws CASPAR_PORT=8076 caspar-client login alice alice@example.com
```

---

## casparctl stats

A realtime multi-section terminal dashboard: container inspect (state/health/
ports/mounts), resource stats with a CPU sparkline, the telemetry snapshot, an
explicit **CHAIN STATS** section (chain/peers/validators/staking/election), and
recent logs. Refreshes every `--interval`.

```bash
casparctl stats [--project-dir PATH] [--interval 2s] [--log-lines 6]
```

It polls `CASPARCTL_TELEMETRY` (default `http://127.0.0.1:9099/telemetry/snapshot`).
Press `Ctrl+C` to exit.

---

## casparctl pprof

Query the node's Rust-native `pprof` runtime profiler (default base URL
`http://127.0.0.1:9999`, override with `--host` or `CASPARCTL_PPROF`).

```bash
casparctl pprof <subcommand> [--host URL] [--seconds N] [--output FILE]
```

| Subcommand | Output |
|------------|--------|
| `runtime` | pid / uptime / thread count / os / arch (JSON) |
| `heap` | per-process memory counters from `/proc` (JSON) |
| `threads` | all OS threads owned by the node (JSON) |
| `flamegraph` | sample CPU for `--seconds N` and render an SVG |
| `profile` | sample CPU and dump a pprof protobuf (`.pb`) |

`--seconds` clamps to `1..=60`. Flamegraph SVGs open in a browser; the `.pb`
loads with `go tool pprof <file>`.

---

## casparctl vms

Manage the node's **pluggable VM types**. Each runtime is a standalone Rust
project in `vms/`; this command selects which ones the node binary supports and
scaffolds new ones. The enable/disable selection is stored in
`vms/vms.state.json`; `sync` regenerates the build-time aggregation crate at
`node/crates/caspar-vm-plugins/` (marked `@generated`), which is the only place
plugins are imported and registered.

```bash
casparctl vms <subcommand> [--vms-dir PATH] [--node-dir PATH]
```

- **`vms list`** — show every VM plugin project found in `vms/` with its
  key, enabled/disabled state, package, version, and name.
- **`vms enable <key> [<key>...]`** — include a VM type in the next build.
- **`vms disable <key> [<key>...]`** — exclude a VM type from the next build
  (warns if you disable the default runtime).
- **`vms sync`** — regenerate the plugin registration code from the current
  selection. Run automatically by `build-dist.sh`; warns if no enabled type
  declares `defaultRuntime`.
- **`vms new <key>`** — scaffold a brand-new VM plugin project (`Cargo.toml`,
  `vm.config.json`, `src/lib.rs`, `src/controller.rs`) ready to implement.

`--vms-dir` is auto-detected (or `CASPAR_VMS_DIR`); `--node-dir` defaults to
`<vms-dir>/../node`.

Typical flow for adding a runtime:

```bash
casparctl vms new myvm      # scaffold vms/myvm
# ... implement src/controller.rs against caspar-vm-sdk ...
casparctl vms list          # verify discovery
casparctl vms sync          # regenerate registration code
./build-dist.sh             # rebuild the node with the plugin compiled in
```

See [VM SDK & Plugins](06-vm-sdk-and-plugins.md) for the implementation contract.

---

## casparctl cluster

Orchestrate the geo-distributed instance mesh (OpenRaft-replicated cluster of
same-origin nodes). Commands talk to the node's cluster HTTP listener (default
`http://127.0.0.1:7440`, override with `--endpoint` or
`CASPARCTL_CLUSTER_ENDPOINT`; set `--token` / `CASPAR_CLUSTER_TOKEN` for a
shared secret).

```bash
casparctl cluster <subcommand> [--endpoint URL] [--token SECRET]
```

- **`cluster status`** — raft state, leader, membership, and a peer RTT table.
- **`cluster init [--include-peers]`** — initialize a pristine cluster on this node.
- **`cluster peers`** — list the peers registered in the cluster config.
- **`cluster nearest`** — peers sorted by measured round-trip latency.
- **`cluster add-peer --id N --addr H:P [--region R] [--lat F] [--lon F] [--learner]`**
  — introduce another instance one by one.
- **`cluster remove-peer --id N`** — remove an instance from the mesh.
- **`cluster promote --ids 1,2,3`** — set the exact raft voter membership.
- **`cluster config list`** — print the full cluster configuration.
- **`cluster config get <key>`** — read one dotted key (e.g. `peers.2.addr`).
- **`cluster config set <key> <value>`** — set one dotted key
  (e.g. `heartbeat_interval_ms 250`).
- **`cluster apply -f cluster.json`** — apply a whole cluster config document.

See [Cluster](08-consensus-federation-cluster.md#geo-distributed-cluster).

---

## Environment variables used by casparctl

| Variable | Used by | Default |
|----------|---------|---------|
| `CASPARCTL_TELEMETRY` | `stats` | `http://127.0.0.1:9099/telemetry/snapshot` |
| `CASPARCTL_PPROF` | `pprof` | `http://127.0.0.1:9999` |
| `CASPAR_VMS_DIR` | `vms` | auto-detected `vms/` |
| `CASPARCTL_CLUSTER_ENDPOINT` | `cluster` | `http://127.0.0.1:7440` |
| `CASPAR_CLUSTER_TOKEN` | `cluster` | (none) |
