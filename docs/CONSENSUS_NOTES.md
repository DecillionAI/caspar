# Consensus throughput & lock-hold analysis

When a cluster runs under heavy mixed load — many `creatures.signal` calls
fanning out to wasm creatures, interleaved with consensus-needing
actions (`/programs/create`, `/programs/deploy`, `/creatures/create`,
…) — the chain can appear "stuck" from the client's point of view:
new signals continue to return `passed: true` (they are dispatched
async via the signaler), but consensus blocks stop advancing and any
write that has to ride the chain pipeline times out at the CLI's 360 s
deadline.

## What is actually happening

`HgHandler::commit_handler` in `drivers/network/chain/blockchain.rs`
runs on the Babble proxy thread for every block the consensus engine
commits. It used to hold `bc.pipeline` for the entire duration of the
pipeline closure:

```rust
let pipeline = bc.pipeline.lock().unwrap();   // guard
if let Some(pipeline) = pipeline.as_ref() {   // ref into the guard
    let _: Vec<String> = pipeline(txs, cb);   // long call, guard still held
}                                              // guard drops
```

The pipeline closure is heavy:

- It calls `handle_chain_packet` for each tx in the block.
- For `base` transactions it invokes secure actions directly (creature
  create, program create/deploy, etc.). Some of those (notably
  `/programs/deploy`) decode a base64 wasm payload, write it to disk,
  build the entity descriptor, and call back into `modify_state` for
  multiple `put_obj` / `put_json` writes.
- For `message` transactions it routes into `globe.handle` which can
  spawn or signal local listeners (each of which may, in turn, drive
  the wasm runtime to invoke `runVm`).

While that work is in flight, the proxy thread is busy and Babble's
consensus loop cannot service the next round. The block at the head
of the queue stays committed but the next "Decided Round" / "Self
Event" pair waits behind it. Under sustained load the gap grows until
clients time out.

## The fix in this branch

`Mutex<Option<PipelineFn>>` is now `Mutex<Option<Arc<PipelineFn>>>`.
`commit_handler` clones the Arc under the lock, drops the guard
immediately, and only then invokes the closure. The Arc keeps the
closure alive for the duration of the call; the mutex is no longer
pinned.

This does not eliminate the underlying back-pressure (the proxy
thread is still synchronous), but it shrinks the critical section to
the time it takes to clone an `Arc` — a few hundred nanoseconds
instead of the tens or hundreds of milliseconds that the heavier txs
take. Concurrent observers (e.g. `register_pipeline`, future readers
that may inspect the slot) are no longer queued behind the running
pipeline.

## What this does NOT fix

- The proxy thread still runs the pipeline synchronously. If the goal
  is true bursty throughput, the per-tx work in `handle_chain_packet`
  should be parallelised behind a worker pool, with `commit_handler`
  acknowledging the block immediately and reconciling state-hashes
  asynchronously. That is a deeper change that needs to be carefully
  serialised against Babble's `state_hash` contract.
- Single-validator setups still produce `Block is not a suitable
  Anchor … trust_count=0` debug lines. Blocks commit locally but
  never become anchor blocks because there is no second signer.
  Functional for development; multi-node federation is required for
  real safety.

## How the symptom maps to specific actions

| client action | needs consensus? | observed under load |
| --- | --- | --- |
| `creatures.me`, `creatures.get` | no (read-only) | always fast |
| `creatures.signal` | no (async dispatch) | always fast |
| `creatures.createMachine` | yes (base trx) | queues behind running pipeline |
| `programs.create` | yes (base trx) | queues behind running pipeline |
| `programs.deploy` | yes (large base trx) | the slowest item — wasm bytes are written, entity built |
| `/creatures/login` | yes (base trx) | queues behind running pipeline |

If consensus appears stuck, check whether the workload is
predominantly signals (which never block consensus) or
consensus-needing writes (which do). Either way, the proxy thread is
the chokepoint.
