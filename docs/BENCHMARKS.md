# Benchmarks 📊

> Source of truth: **`reports/final/`** (run **2026-05-29**). Reproduce with
> `./bench-all.sh` (writes a `workflow_report.md` + JSON results); curated runs
> are archived under `reports/`.

## Setup

- **One shard** = three local nodes (`node1:8074`, `node2:8174`, `node3:8274`)
  sharing a single Babble consensus group, plus one QuestDB instance (`:8812`).
- Single-CPU sandbox; `caspar-node` v0.1.0, WasmEdge 0.14.0, RocksDB, Babble.
- Each node loaded **37 WASM creatures** (111 across the shard), 304–379 KB each,
  ≈ **11.6 MB** per node; mean deploy time ≈ 5.5 s/creature, 100% deployed.
- Driver: `sdk/caspar_client.py`, nine workflow suites, **154 steps** total.

## Correctness

**154 / 154 steps passed (100%)** across nine suites.

| Suite | Steps | Pass |
|-------|------:|-----:|
| chain (create/shard/register/submit) | 12 | 12 |
| stores (create/list/join/get) | 12 | 12 |
| storage (upload/download/delete) | 9 | 9 |
| invites (create/list/cancel) | 9 | 9 |
| elpify-chain (stake→commit→reveal→elect→exec) | 48 | 48 |
| cross-creature orchestration | 18 | 18 |
| throughput burst | 33 | 33 |
| federation (cross-node propagation) | 4 | 4 |
| concurrent load (WASM + MASM) | 9 | 9 |

## Sequential Throughput (n = 20 per op, single TLS connection)

| Operation | n1 | n2 | n3 | Mean |
|-----------|---:|---:|---:|-----:|
| chain:submitBaseTrx | 10.44 | 10.50 | 10.16 | **10.37** |
| elpify-chain:status | 8.58 | 10.44 | 9.19 | 9.40 |
| storage:upload | 9.75 | 9.91 | 9.90 | 9.85 |
| storage:download | 9.75 | 9.33 | 8.14 | 9.07 |
| stores:create | 6.79 | 9.44 | 8.64 | 8.29 |
| stores:list | 7.91 | 9.37 | 9.36 | 8.88 |
| invites:listUserInvites | 9.27 | 9.20 | 9.16 | 9.21 |
| invites:listStoreInvites | 9.06 | 8.77 | 8.91 | 8.91 |
| elpify:executeTrx (MASM) | 8.45 | 8.83 | 7.67 | 8.32 |

**Peak: 10.50 ops/s** (`chain:submitBaseTrx`, node2, p50 = 93.3 ms). The
≈95 ms median consensus round sets the per-connection ceiling
(`1000/95 ≈ 10.5 ops/s`); the fastest single request observed was 84 ms.

## Latency (ms, all nodes pooled)

| Operation | p50 | p95 | p99 | max |
|-----------|----:|----:|----:|----:|
| invites (create/list) | 101–117 | 184 | 184 | 184 |
| chain:submitBaseTrx | 92–96 | 120 | 120 | 120 |
| storage:upload/download | 97–120 | 178 | 178 | 178 |
| stores:create | 112–136 | 261 | 261 | 261 |
| stores:list | 110–122 | 176 | 176 | 176 |
| chain:create (first call) | 95–126 | 126 | 126 | 126 |
| elpify:electionTick | 112–164 | 164 | 164 | 164 |

## elpify-chain Election (per-phase, all nodes)

| Phase | Mean (ms) | Std (ms) |
|-------|----------:|---------:|
| stake (3 validators) | 113.7 | 28.6 |
| commit (3 validators) | 107.6 | 10.6 |
| reveal (3 validators) | 117.7 | 16.8 |
| electionTick | 132.0 | 27.8 |
| status confirmation | 115.5 | 0.5 |
| executeTrx (MASM) | 126.6 | 12.7 |

The warm electionTick cost is ≈112–120 ms (one consensus round); the 132 ms mean
is inflated by a single cold first election on node1 (164 ms).

## Mixed Workload (70% reads / 30% writes, n = 30 interleaved)

| Node | TPS | Mean (ms) | p50 (ms) |
|------|----:|----------:|---------:|
| node1 | 8.99 | 111.1 | 109.4 |
| node2 | 8.79 | 113.6 | 114.9 |
| node3 | 8.23 | 121.3 | 122.6 |

Zero errors. Under interleaving the read-path p50 rises to ~109–123 ms.

## Concurrent Load (node1; `/proc`-sampled resources)

| Engine | C | TPS | p50 (ms) | p99 (ms) | CPU % | RSS (MB) | OK |
|--------|--:|----:|---------:|---------:|------:|---------:|---:|
| wasm | 1 | 9.90 | 99 | 123 | 303 | 3962 | 100% |
| wasm | 4 | 6.48 | 623 | 1136 | 198 | 3988 | 100% |
| wasm | 8 | 6.97 | 1135 | 1905 | 196 | 3990 | 100% |
| wasm | 16 | 6.54 | 2322 | 4092 | 197 | 4073 | 100% |
| wasm | 32 | 4.22 | 7462 | 13275 | 186 | 4112 | 100% |
| masm | 1 | 7.55 | 140 | 148 | 321 | 4107 | 100% |
| masm | 2 | **11.31** | 168 | 224 | 326 | 4144 | 100% |
| masm | 4 | 9.04 | 310 | 946 | 258 | 4155 | 100% |
| masm | 8 | 7.56 | 1020 | 1515 | 225 | 4163 | 100% |

**MASM/STARK peaks at 11.31 proofs/s @ C = 2.** Every request succeeds at every
concurrency level. Peak RSS ≈ 4.16 GB.

## Notes & Scope

- All figures measure **one shard**. Shards are independent Babble groups, so
  aggregate network throughput is additive: `TPS_network = S × TPS_shard`.
- Sequential throughput is consensus-bound (not CPU-bound) and does not scale
  with cores; concurrent throughput scales with cores.
- STARK figures are a **CPU-only** lower bound (the host had no GPU).
- The Docker / Firecracker / `pc` workflows need a Docker + micro-VM daemon and
  are not exercised in the CPU-only sandbox.
