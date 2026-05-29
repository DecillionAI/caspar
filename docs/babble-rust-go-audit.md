# Babble Rust ↔ Go translation audit

Audit of the Rust port (`node/src/drivers/network/chain/`) against the
original Go implementation (`node.old/node/src/drivers/network/chain/`),
performed as part of the work landing on PR #102.

The port intentionally diverges from the original on two axes that the
user has explicitly called out and that should NOT be flagged as bugs:

1. **Gossip is event-driven, not periodic.** Every Go path that
   relied on `time.Sleep(heartbeat) → next babble() tick →
   dispatch_gossip()` to propagate new content must, in Rust, fire
   `Node::trigger_gossip()` explicitly. A heartbeat-only fallback
   still exists (`control_timer.tick_rx`) but it is the safety net,
   not the steady-state driver.
2. **Chain transport is plain TCP.** The Go side had separate WebRTC,
   WAMP, and RPC stream layers; the Rust side has only
   `chain/net/tcp_stream_layer.rs`. Go files that map to the WebRTC /
   WAMP / RPC paths are dead code from the Rust port's perspective.

The audit was performed by three parallel reviewer agents covering the
highest-signal areas:

* **R5** — `peers/json_peer_set.{rs,go}`, `peer_set.{rs,go}`,
  `peer.{rs,go}` (the suspected `0X` casing bug — strongest a-priori
  candidate for the "node stuck in JOINING forever" symptom).
* **R1** — `node/core.{rs,go}` (consensus state, head/seq tracking,
  internal-transaction processing).
* **R2** — `node/node.{rs,go}` and `node/node_rpc.{rs,go}` (state
  machine and event-driven gossip trigger coverage).

Verdicts: `FIX_NOW` (real bug, fixed in this PR), `DEFER` (suspected
divergence whose impact is unclear or whose fix needs more careful
work), `NOT_A_BUG` (Rust and Go are functionally equivalent — false
alarm), `INTENTIONAL_EVENT_DRIVEN` (Go's periodic-gossip path that has
intentionally no Rust counterpart), `INTENTIONAL_TCP_ONLY` (Go path
specific to WebRTC/WAMP/RPC that has intentionally no Rust
counterpart).

## Summary table

| Module | Symbol | Rust file:line | Go file:line | Verdict | Action |
| ------ | ------ | -------------- | ------------ | ------- | ------ |
| peers  | `cleanse_peer_set` | json_peer_set.rs:66-71 | json_peer_set.go:72-75 | NOT_A_BUG | none — trace below |
| peers  | `Peer::id()` / FNV input | peer.rs:36-38 | peer.go:36-41 | NOT_A_BUG | none |
| node   | gossip trigger after EagerSync ingestion | node_rpc.rs:144-169 | node_rpc.go:193-216 | **FIX_NOW** | **fixed in commit 28845f7** |
| node   | gossip trigger after JoinRequest enqueue | node_rpc.rs:209-282 | node_rpc.go:292-310 | **FIX_NOW** | **fixed in commit 28845f7** |
| node   | gossip trigger after `add_transaction` | node.rs:369 | node.go:365-367 | NOT_A_BUG | Rust already calls `trigger_gossip()` here |
| node   | gossip trigger after `pull → sync` | node.rs:514-541 | node.go:519-553 | NOT_A_BUG | Rust already calls `trigger_gossip()` here |
| node   | gossip trigger on Sync / FastForward RPCs | node.rs:340-376 | n/a | INTENTIONAL_EVENT_DRIVEN | Sync/FastForward are read-only on the responder; nothing new to gossip |
| core   | `process_commit` error semantics | core.rs:471-505 | core.go:487-538 | NOT_A_BUG | both skip the bookkeeping block when the commit-callback returns Err, then return the err — observable behaviour matches (see below) |
| core   | `record_heads` extra solo self-event | core.rs:282-287 | core.go:275-290 | DEFER | likely intentional given the event-driven gossip change, but no test pins the semantics — see "deferred items" |
| core   | sync logging removed | core.rs:202-260 | core.go:256-262 | NOT_A_BUG | cosmetic — Rust dropped the per-cycle pool log line |

## R5 — peers (`json_peer_set`, `peer_set`, `peer`)

R5 was the highest-priority reviewer because a mismatch in `Peer.id()`
between Rust and Go would deterministically cause every node to fail
the `core.peers.by_id.contains_key(validator.id())` check at boot and
sit in the `JOINING` state forever — which is the symptom the cluster
exhibited before the `caspar-keygen → key.pub` fix landed earlier in
this PR.

**Verdict: NOT_A_BUG.**

Both languages run a `cleanse_peer_set` step on JSON deserialisation
that normalises every `PubKeyHex` to uppercase with a `0X` prefix.
Trace through all four representative inputs:

```
Input "0xabc"   → uppercase "0XABC" → strip "0X" → "ABC" → "0X" + "ABC" = "0XABC"
Input "abc"     → uppercase "ABC"   → strip "0X" → "ABC" → "0X" + "ABC" = "0XABC"
Input "0XABC"   → uppercase "0XABC" → strip "0X" → "ABC" → "0X" + "ABC" = "0XABC"
Input "0xABC"   → uppercase "0XABC" → strip "0X" → "ABC" → "0X" + "ABC" = "0XABC"
```

Rust uses `String::to_uppercase()` + `strip_prefix("0X").unwrap_or(&upper)`
+ `format!("0X{}", trimmed)`; Go uses `strings.ToUpper` +
`strings.TrimPrefix("0X")` + concatenation. The outputs match
byte-for-byte. Both `Peer::pub_key_bytes()` and Go's
`Peer.PubKeyBytes()` use case-insensitive hex decoders, so even an
unnormalised input would still produce the same byte slice and the
same FNV-1a hash; the cleanse step is defence in depth, not a
correctness load-bearing call.

R5's full table covers eight related symbols (PeerSet constructor,
JSONPeerSet read/write paths, public-key encoding, hex
encoding/decoding, test coverage) and finds all of them equivalent.

## R2 — node state machine and gossip triggers

R2's brief was the most important from the user's perspective: the
Rust port replaces Go's periodic gossip with explicit
`trigger_gossip()` calls, so every Go path that ended in
`controlTimer.resetCh ← _; …; next babble() tick → dispatch_gossip()`
must now have a corresponding Rust path that explicitly wakes the
gossip loop.

R2 found **two FIX_NOW divergences**, both in `node_rpc.rs`:

### FIX 1: `process_eager_sync_request` missing `trigger_gossip()`

`node_rpc.rs:144-169`, mirrored by `node_rpc.go:193-216`.

The EagerSync handler ingests new hashgraph events from a peer via
`sync_with_core`. In Go, the function returns into `doBackgroundWork`
which calls `resetTimer`, the next `babble()` tick fires
`dispatch_gossip()`, and the just-ingested events propagate to other
peers on the next gossip round. In Rust the babble loop sleeps on
`gossip_trigger_rx` until something fires `trigger_gossip()` — and
nothing did. The ingested events sat in the local store until the
fallback heartbeat ticked, silently delaying consensus propagation
and, under load, occasionally missing rounds.

**Fixed in commit `28845f7`**: `process_eager_sync_request` now calls
`self.trigger_gossip()` on the success path. The fix is gated on
`!err` so a sync error does not waste a gossip cycle.

### FIX 2: `process_join_request` missing `trigger_gossip()`

`node_rpc.rs:209-282`, mirrored by `node_rpc.go:292-310`.

The JoinRequest handler enqueues a new peer's `InternalTransaction`
into `core.add_internal_transaction`. The transaction can only be
accepted by consensus once the rest of the validator set has seen it,
which only happens if we gossip it out — and in the Rust event-driven
model that requires an explicit `trigger_gossip()`. Without it the
joiner sits in `JOINING` until our heartbeat ticks. When several
joiners hit at once that delay compounds into the
`"Cannot join: Not in Babbling state"` cascade the node logs already
show.

**Fixed in commit `28845f7`**: `process_join_request` now calls
`self.trigger_gossip()` immediately after `add_internal_transaction`,
before waiting on the JoinPromise receiver.

### Other paths audited and cleared

* `node.rs:369` — `add_transaction` already calls `trigger_gossip()`
  inline. No change needed.
* `node.rs:514-541` — `pull → sync_with_core` already calls
  `trigger_gossip()` if events were received. No change needed.
* `node.rs:340-376` — Sync and FastForward RPCs do NOT call
  `trigger_gossip()`. This is correct (`INTENTIONAL_EVENT_DRIVEN`):
  both are read-only on the responder side, so the responder has
  nothing new to gossip out.
* `core.rs:454-461` — `drain_commits()` is invoked synchronously
  inside `sync_with_core` after every event insertion. The current
  `trigger_gossip()` at the EagerSync site (after the fix above)
  covers the case where the committed block changes peer-set state
  the network needs to see. R2 marks this DEFER pending a regression
  test that pins the semantics.

## R1 — node/core

### `process_commit` error semantics — NOT_A_BUG

R1 initially flagged Rust's `process_commit` as FIX_NOW: on a commit
callback error Rust returns `Err` immediately, whereas Go (it
claimed) continues to do the anchor-block / peer-receipt bookkeeping.
Cross-checking the Go source directly (`core.go:487-538`) shows the
opposite: Go also gates the entire bookkeeping block on
`if err == nil { ... }` (line 508-535) and returns the err at the
end. Functional observable behaviour is identical:

```
Rust on commit-callback Err: log error → return Err
Go on commit-callback Err: log error → log spurious "Commit response"
                            line with empty receipts → return err
```

The only difference is that Go emits an additional
`"Commit response"` log line referencing the zero-valued
`commitResponse` (because Go declares `commitResponse, err := ...`
unconditionally) and that line shows up regardless of error. Rust
skips that log line. This is a cosmetic / diagnostic difference, not
a correctness divergence. Downgraded to NOT_A_BUG.

### `record_heads` extra solo self-event — DEFER

R1 flagged Rust's `record_heads` for adding a solo
`add_self_event("")` (lines 282-287) when any transaction pool is
non-empty, where Go iterates only the `heads` map. This is plausibly
**intentional** given the event-driven gossip change: in Go's
periodic-tick model `recordHeads` was called every tick whether or
not new events were available, so it could afford to do nothing when
`heads` was empty. In Rust the function may not be called for a long
time after the last event, so producing one synthetic head to carry
the pending transactions through consensus is a reasonable
adjustment.

R1 has no regression test pinning either semantic, and removing the
solo event without one risks losing events under low-traffic
conditions. **Deferred** to a follow-up PR that lands together with
a property test exercising the
`{has_pending_txs ∧ no_real_head}` corner.

## Deferred items (follow-up PRs)

1. **`record_heads` solo self-event semantics** — see R1 above. Needs
   a regression test before either path can be confidently changed.
2. **`drain_commits` → gossip trigger** — R2 DEFER. The EagerSync /
   JoinRequest fixes already cover the most-visible window; a deeper
   audit of `process_accepted_internal_transactions` (which can
   change peer-set state at `round_received + 6`) should land with
   a test that demonstrates peer-set propagation timing.
3. **Bounded babble RPC retries on the responder side** — out of
   scope for this audit but worth noting: only the dialer side
   currently retries (in `NetworkTransport::get_conn`, also added in
   this PR). The responder gives up on the first malformed-frame
   error; a malicious or buggy peer can disrupt one round per
   request, no more, so this is low priority.

## Methodology notes

* The audit is source-text comparison only. The Rust port has its
  own unit-test suite under `node/src/drivers/network/chain/`, but
  no behavioural cross-check against the Go binary was performed
  (would require setting up a parallel test harness, out of scope
  for this PR).
* The three reviewers ran in parallel as read-only agents; each
  wrote findings to a temp file, this document is the merge. Any
  divergence the reviewers flagged but the merger downgraded (e.g.
  `process_commit`) is annotated with the rationale above.
* Coverage is intentionally narrow: `peers`, `node`, and `core` are
  the highest-blast-radius modules. `hashgraph/` (divide_rounds,
  decide_fame, etc.) was NOT audited in this pass; if the cluster
  exhibits consensus stalls after the FIX_NOW changes land in CI,
  a follow-up audit of that module is the next step.
