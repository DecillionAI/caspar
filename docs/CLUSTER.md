# Geo-Distributed Caspar Cluster

A Caspar deployment can run as a **mesh of instances of the same origin
(authority)** spread across the globe, working the way edge networks do:
every instance serves the requests that reach it (lowest geographic
latency), and the resulting state is propagated to every other instance
through an **OpenRaft**-replicated log.

```
            ┌────────────────────────  raft log (OpenRaft)  ───────────────────────┐
            │                                                                      │
   ┌────────┴────────┐            ┌─────────────────┐            ┌─────────────────┴───┐
   │  caspar-eu (1)  │ ◀────────▶ │  caspar-us (2)  │ ◀────────▶ │  caspar-ap (3)      │
   │  eu-west        │  raft RPC  │  us-east        │  raft RPC  │  ap-south           │
   └────────▲────────┘            └────────▲────────┘            └──────────▲──────────┘
            │ nearest instance            │ nearest instance                │
         EU clients                    US clients                       APAC clients
```

## What replicates

| Source                                   | Replicated? | Mechanism |
|------------------------------------------|-------------|-----------|
| Shell API writes (users, machines, programs, stores, …) | always      | write-set → `KvBatch` raft entry |
| Creature deployed with `distribution: "cluster"`        | always      | full artifact → `Deploy` raft entry |
| VM state of cluster-distributed creatures (storage ops, per-VM transactions) | always | write-set → `KvBatch` raft entry |
| VM state of local-mode creatures (`distribution: "local"`, the default) | never | commits on the receiving instance only |
| Cluster-wide operator knobs (`extra.*` config keys)      | always      | `ConfigPut` raft entry |

Execution is **edge-first**: the instance that receives a request executes
it immediately against its local state (reads never leave the instance),
then the committed write-set is proposed to the raft log and applied on
every other instance. Followers forward proposals to the current leader
automatically, so requests can land on any instance.

## Enabling cluster mode on a node

Cluster mode is off by default — a standalone node behaves exactly as
before. Enable it with the config file at
`<STORAGE_ROOT_PATH>/cluster/cluster.json` (or `CLUSTER_CONFIG_PATH`),
or with environment variables:

```bash
CLUSTER_ENABLED=true
CLUSTER_NODE_ID=1                       # unique u64 per instance
CLUSTER_NODE_NAME=caspar-eu
CLUSTER_REGION=eu-west
CLUSTER_LISTEN_ADDR=0.0.0.0:7440        # raft RPC + orchestration API
CLUSTER_ADVERTISE_ADDR=caspar-eu.example.com:7440
CLUSTER_AUTH_TOKEN=shared-secret        # optional, protects the listener
```

On first start with no persisted raft state the node initializes itself as
a single-voter cluster and becomes leader, ready to accept peers.

## Orchestration with `casparctl cluster`

Introduce the other instances **one by one** (domain or IP):

```bash
# on / against the current leader instance
casparctl cluster add-peer --id 2 --addr caspar-us.example.com:7440 --region us-east
casparctl cluster add-peer --id 3 --addr 203.0.113.7:7440 --region ap-south --lat 12.97 --lon 77.59
casparctl cluster add-peer --id 4 --addr caspar-edge.example.com:7440 --learner   # read replica

casparctl cluster status      # leader, term, membership, replication, RTT table
casparctl cluster peers       # configured peers
casparctl cluster nearest     # peers sorted by measured round-trip latency
casparctl cluster remove-peer --id 4
casparctl cluster promote --ids 1,2,3   # set the exact voter set
```

…or configure the **whole cluster from one document**:

```bash
casparctl cluster apply -f cluster.json
```

```json
{
  "region": "eu-west",
  "heartbeat_interval_ms": 500,
  "election_timeout_min_ms": 1500,
  "election_timeout_max_ms": 3000,
  "max_payload_entries": 300,
  "snapshot_logs_since_last": 5000,
  "rtt_probe_interval_secs": 15,
  "peers": {
    "1": { "id": 1, "addr": "caspar-eu.example.com:7440", "region": "eu-west",  "voter": true },
    "2": { "id": 2, "addr": "caspar-us.example.com:7440", "region": "us-east",  "voter": true },
    "3": { "id": 3, "addr": "caspar-ap.example.com:7440", "region": "ap-south", "voter": true }
  },
  "extra": { "maintenanceWindow": "02:00-04:00Z" }
}
```

Every knob in that document is also **individually** readable and writable:

```bash
casparctl cluster config list
casparctl cluster config get peers.2.addr
casparctl cluster config set heartbeat_interval_ms 250
casparctl cluster config set peers.2.region "us-west"
casparctl cluster config set extra.maintenanceWindow '"03:00-05:00Z"'   # replicated cluster-wide
```

Global flags: `--endpoint http://host:7440` (env `CASPARCTL_CLUSTER_ENDPOINT`)
and `--token secret` (env `CASPAR_CLUSTER_TOKEN`).

## Distributed vs local creature deployment

The `/programs/deploy` shell action now takes a distribution choice:

```json
{
  "machineId": "…", "entityId": "…", "entityType": "docker",
  "payload": "<base64>",
  "distribution": "cluster"        // or "local" (default); bool alias: "distributed": true
}
```

* **`"cluster"`** — the creature program (artifact bytes included) is
  written to the raft log and materialized on **every instance**: files are
  saved, entity/program records and runtime links restored, the VMM
  listener registered, and — for build-on-deploy runtimes such as docker —
  the image built locally on each instance. Any instance can then serve the
  creature's VM requests (Cloudflare-Workers-style geo execution), and all
  the VM's key-value mutations / per-VM transactions are propagated back
  through the consensus.
* **`"local"`** — the creature exists only on the instance that received
  the deploy, and nothing about its VM execution enters the consensus.

The deploy response echoes the effective scope: `{"distribution": "cluster"}`.

## Design notes

* **Local-first, then consensus.** A request is executed by the receiving
  instance against its local RocksDB; the committed write-set is then
  proposed to the raft log (`KvBatch`). Instances apply batches from other
  origins directly into their store, and skip their own (already-applied)
  batches. This gives edge-latency execution with cluster-wide convergence,
  at the cost of last-writer-wins semantics between instances that mutate
  the same key concurrently.
* **Raft storage.** Log + state-machine metadata live in a dedicated
  RocksDB at `<storage_root>/cluster/raft-db` (column families `meta`,
  `logs`, `sm`), separate from the node's main database.
* **Snapshots & new joiners.** Snapshots carry the state machine metadata
  and the replicated config store; bulk application state is persisted by
  each instance at apply time. Join new instances before log compaction
  trims history (`snapshot_logs_since_last`, default 5000), or seed them
  from a storage backup of an existing instance first.
* **Security.** Set `auth_token` (or `CLUSTER_AUTH_TOKEN`) so every raft
  and orchestration call must present `x-caspar-cluster-token`. Run the
  listener over a private network / VPN or terminate TLS in front of it for
  cross-datacenter links.
* **Geo routing.** Each instance probes its peers every
  `rtt_probe_interval_secs` and serves the latency-sorted table at
  `/cluster/nearest` (`casparctl cluster nearest`) — feed it to your
  GeoDNS/anycast layer to steer clients to the closest instance.
* **What stays node-local.** The hashgraph/babble chain, federation
  transport, telemetry, and QuestDB time-series logs keep their existing
  per-node behaviour; the raft mesh replicates the shell/VM key-value
  state layer described above.
