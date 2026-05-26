# Consensus throughput & quiet-state behaviour

When a fresh node boots and then sits without any consensus-bound
client traffic, the babble log goes completely silent after the
`BABBLING` banner. No `Self-Event`, no `Decided Round`, no `Commit`.
A casual observer reading the log will conclude that consensus has
deadlocked. It hasn't.

## Why the log goes quiet

`Node::babble()` waits on the heartbeat tick. On every tick it calls
`dispatch_gossip()`, which for a single-validator setup picks no
peer (the peer selector excludes self) and falls through to
`monologue()`. `monologue()` is:

```rust
fn monologue(&self) -> Result<()> {
    let mut core = self.core.lock().unwrap();
    if core.busy() {
        if let Err(e) = core.add_self_event("") { … }
        if let Err(e) = core.process_sig_pool() { … }
    }
    Ok(())
}
```

`add_self_event` is the call that emits the `Created Self-Event`
debug line. It is guarded by `core.busy()`. With no pending
transactions and no signatures in the pool, every tick is a silent
no-op. The chain is alive — it just has nothing to do, so it doesn't
log.

Drop a single consensus-bound write at it (any
`secure_action` whose input declares `origin == "global"`, e.g.
`creatures.createMachine`, `programs.deploy`, `creatures.create`)
and the log lights up: `Adding Transaction → Created Self-Event →
Decided Round → Commit block=N`.

## What is actually slow under load

Under sustained mixed load — many `creatures.signal` calls fanning
out to wasm creatures, interleaved with consensus-needing actions
— the chain pipeline still has to serialise all
consensus-bound writes through the babble proxy thread. That thread
runs `commit_handler`, which dispatches each block's transactions
through `handle_chain_packet`. For heavier txs (notably
`/programs/deploy`, which decodes a base64 wasm payload, writes it
to disk and builds the entity descriptor), the pipeline closure
takes tens or hundreds of milliseconds. While that closure runs,
the next block cannot be produced.

The defensive change in `f58c856` switches
`Blockchain.pipeline` from `Mutex<Option<PipelineFn>>` to
`Mutex<Option<Arc<PipelineFn>>>` and clones the Arc under the lock
before invoking the closure. Holding the lock through the closure
was never strictly required (`register_pipeline` is called once at
boot), but it widened the critical section. The Arc clone shrinks
that to a few hundred nanoseconds.

## Quick reference: which actions need consensus?

| action | needs consensus? | observable signal in node.log |
| --- | --- | --- |
| `creatures.me`, `creatures.get`, `creatures.list` | no (read-only) | none |
| `creatures.signal` | no (async dispatch via signaler) | none — wasm gas costs only |
| `creatures.login` (dev mode) | no (handler calls /creatures/create directly via fetch_action) | none |
| `creatures.createMachine` | yes (CreateInput.origin = "global") | Adding Transaction + Commit |
| `creatures.create` | yes | Adding Transaction + Commit |
| `programs.create` | yes (CreateMachineInput.origin = "global") | Adding Transaction + Commit |
| `programs.deploy` | yes — and slow (wasm bytes + entity build) | Adding Transaction + Commit, big WasmEdge gas log block |

When you suspect the chain has "stuck", first push one consensus-
bound write at it. If the log lights up and a new `Commit block=N`
appears within a couple of seconds, the chain was simply idle.

## What this branch does NOT fix

- Single-validator runs always log `Block is not a suitable Anchor
  … trust_count=0` after every commit. Blocks commit locally but
  never become anchor blocks because there is no second signer.
  Functional for development; multi-node federation is required for
  real safety.
- The babble proxy thread still runs the pipeline synchronously.
  Truly bursty throughput would require splitting per-tx work behind
  a worker pool with state-hash reconciliation, which is a larger
  change.
