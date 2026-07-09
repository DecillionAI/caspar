# Geo-Distributed Caspar Cluster

The Caspar network is a **two-layer hierarchy**:

1. **Outer layer — the network of nodes.** Independent Caspar *nodes*
   (different origins/authorities) compose the decentralized network
   through the existing hashgraph BFT chain and the federation bridge.
   Nothing in this document changes that layer.
2. **Inner layer — the instances of one node.** Each logical node can now
   be *made of multiple replicated instances*, each potentially on a
   separate server anywhere in the world. This inner layer is the
   OpenRaft cluster described here: same origin, same authority, one
   replicated state.

The inner layer works the way edge networks do: every instance serves the
requests that reach it (lowest geographic latency), and the resulting
state is propagated to every other instance of that node through an
**OpenRaft**-replicated log. From the outer network's point of view the
instances are still *one node* — chain and federation identity are
unchanged.

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
CLUSTER_BOOTSTRAP=true                  # ONLY on the first (seed) instance
CLUSTER_NODE_ID=1                       # unique u64 per instance
CLUSTER_NODE_NAME=caspar-eu
CLUSTER_REGION=eu-west
CLUSTER_LISTEN_ADDR=0.0.0.0:7440        # raft RPC + orchestration API
CLUSTER_ADVERTISE_ADDR=caspar-eu.example.com:7440
CLUSTER_AUTH_TOKEN=shared-secret        # optional, protects the listener
```

**Exactly one instance of a brand-new cluster boots with
`CLUSTER_BOOTSTRAP=true`** (or `"bootstrap": true` in `cluster.json`): it
initializes itself as the first voter and becomes leader. Every joining
instance keeps `bootstrap` false and starts *pristine* — it is pulled into
the cluster when the seed runs `casparctl cluster add-peer` (a pristine
instance must never self-initialize, or it would form a second,
un-mergeable single-node cluster). `casparctl cluster init` initializes a
seed manually when you prefer not to set the env flag.

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
  state layer described above. In the two-layer hierarchy this means the
  outer network (node ↔ node) still converges through the hashgraph
  chain and federation, while the inner layer (instance ↔ instance of
  one node) converges through OpenRaft. Chain packets applied by an
  instance commit through the same transaction wrapper, so their state
  also reaches that node's other instances.

## Test coverage

`node/src/drivers/cluster/tests.rs` boots **three real instances in one
process** (own RocksDBs, raft logs, HTTP listeners) and exercises the
production surface end-to-end: cluster formation via `add-peer`, shell
write-set replication through the real transaction-commit hook, delete
replication, local-scope suppression for non-distributed VM commits,
distributed creature deployment (artifact files + program/entity records
+ runtime links + VMM listener registration + image build on every
instance, with the origin skipping re-application), config get/set,
ping/status/nearest telemetry, RTT probing, follower→leader proposal
forwarding, **leader failover** (surviving quorum re-elects and keeps
replicating), and auth-token enforcement (401 without the shared
secret). Run with `cargo test -p caspar-node cluster`.
