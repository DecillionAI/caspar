# 08 — Consensus, Federation & Cluster

This page covers the distributed-systems layers: the hashgraph consensus chain,
the STARK-attested validator election, federation between deployments, sharding,
and the geo-distributed instance mesh. Each is its own section.

---

## Hashgraph chain (Babble)

Consensus is an **embedded Babble (Rust) hashgraph** — leaderless, asynchronous,
Byzantine-fault-tolerant. It lives in `node/src/drivers/network/chain` and runs
as a service on `BLOCKCHAIN_API_PORT`. The node registers a `commit_handler`
that receives ordered blocks of transactions and fans them out to application
state.

**Consensus routing** decides what goes through the chain, by the input's
`origin` field (not the route name):

- `origin == "global"` → consensus-bound; produces `Adding Transaction →
  Commit block=N`.
- `origin == ""` → local (reads, `creatures.signal`, dev `login`); no chain
  activity.

The chain HTTP surface exposes `/stats`, `/peers`, `/validators`, `/history`,
which the telemetry collector proxies into the snapshot's `chain`/`validators`/
`staking`/`election` fields (visible in `casparctl stats`).

Measured behaviour (reference run `reports/final`): median consensus round
≈95 ms (84 ms floor), peak sequential throughput ~10.5 ops/s for
`chain:submitBaseTrx`.

---

## elpify-chain (STARK validator election)

The elpify-chain creature runs a five-phase **commit-reveal Proof-of-Stake**
election entirely inside WASM, attested by Miden **STARK** zero-knowledge
proofs:

1. **Stake** — record `(id, stake)`.
2. **Commit** — validators submit `h = H(s ‖ n)`.
3. **Reveal** — validators submit `(s, n)`; the creature checks the hash and
   accumulates VRF input.
4. **electionTick** — a MASM program runs in the elpify VM; the Miden prover
   (the `elpify-lang` crate) emits a STARK proof; winners are selected and the
   proof is broadcast via `signalGroup`.
5. **Validator consensus** — electors verify the succinct proof and finalise.

Proving is asymptotically `O(n log² n)` and verification `O(log² n)`, so adding
a validator is far cheaper than generating the proof. See the `elpify` runtime
in [VM Types](07-vm-types-and-implementation.md#elpify--elpify-provable-vm).

---

## Federation

Each node runs a federation transport on `FEDERATION_API_PORT` that validates
the **signature and origin** of inbound packets against a known-origins registry
before admitting them to the local action router. Outbound calls propagate
creature updates and chain events to registered peers.

This enables **cross-deployment creature composition** and **cross-shard event
delivery without replicating consensus history** — a node can compose with
creatures on an independent deployment by federating requests/updates rather
than sharing a chain.

---

## Sharding

A **shard** is a self-contained three-node Babble consensus group with no
cross-shard coordination, so aggregate throughput is additive:

```text
TPS_network = S × TPS_shard
```

The chain API exposes `chains/createShard` and `chains/registerNode`; each shard
runs its own Babble instance. A single shard is the three local nodes
(8074 / 8174 / 8274) sharing one Babble group.

---

## Geo-distributed cluster

Instances of the **same origin** form an edge-style global cluster replicated
with **OpenRaft**. Properties:

- Shell-API state is available on **every** instance.
- Creatures can opt into cluster-wide distributed deployment
  (`distribution: "cluster"`); distributed VM state propagates through
  consensus, while local-mode VMs stay node-local.
- The cluster listener defaults to `127.0.0.1:7440`.

Orchestrate it with `casparctl cluster …`
(full reference in [Casparctl](04-casparctl.md#casparctl-cluster)):

```bash
casparctl cluster status                                 # leader / membership / RTT
casparctl cluster init --include-peers                   # bootstrap on this node
casparctl cluster add-peer --id 2 --addr eu.example.com:7440 --region eu-west
casparctl cluster nearest                                # peers by measured RTT
casparctl cluster promote --ids 1,2,3                    # set raft voters
casparctl cluster config set heartbeat_interval_ms 250   # one knob
casparctl cluster apply -f cluster.json                  # whole-cluster config
```

Set `CASPAR_CLUSTER_TOKEN` (or `--token`) when the mesh uses a shared secret,
and `CASPARCTL_CLUSTER_ENDPOINT` (or `--endpoint`) to target a remote listener.

---

## How the layers relate

- **Chain (Babble)** orders consensus-bound writes within a shard.
- **Shards** scale throughput horizontally with no coordination between them.
- **elpify-chain** elects the validator set that secures the chain, proven by
  STARKs.
- **Federation** links independent deployments/shards for composition and event
  delivery without sharing consensus.
- **Cluster (OpenRaft)** replicates one origin's shell state and opt-in VM state
  across geo-distributed instances for edge availability.
