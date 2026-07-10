# Rate Limiting

Caspar throttles **client → node** requests with a single, protocol-agnostic
token-bucket limiter. Every client-facing transport shares the same limiter
instance, so a client's quota is **unified across protocols** — it cannot be
multiplied by spreading load over more than one transport.

## Where it sits

The limiter is a driver port, [`IRateLimiter`], hung off the node's shared
`ITools` bundle and reachable from every driver through `ICore`. All three
client-facing transports consult it before a request enters the action
pipeline:

| Transport | File | Identity used |
|---|---|---|
| TLS-TCP client | `node/src/drivers/network/client/tcp.rs` | verified `user_id`, else peer IP |
| TLS WebSocket client | `node/src/drivers/network/client/ws.rs` | verified `user_id`, else peer IP |
| VMM HTTP ingress | `node/src/drivers/vmm/network/ingress.rs` | peer IP (always anonymous) |

Because they all call one `ITools::rate_limiter()` instance keyed on identity
(never on the wire the request arrived on), a user that fans requests out over
TCP **and** WebSocket **and** HTTP still draws from a single bucket.

```
   TCP ─┐
   WS  ─┼──►  ITools::rate_limiter()  ──►  one bucket per identity
  HTTP ─┘        (shared instance)
```

## Algorithm: token bucket

A token bucket enforces a **sustained rate** while still permitting short
**bursts**:

- each identity's bucket holds up to `burst` tokens (starts full),
- tokens refill continuously at `rate_per_sec`,
- each request spends one token; a request with no token available is rejected
  with a `retry_after` hint.

This is the standard production choice: it tolerates the bursty request patterns
real apps produce (a flurry on screen-open, then idle) without the
double-rate boundary problem of fixed windows.

## Tiers

| Tier | Keyed by | Purpose | Default |
|---|---|---|---|
| **Authenticated** | verified `user_id` | fair per-user quota for signed-in clients | 50 rps, burst 100 |
| **Anonymous** | peer IP | pre-auth traffic (handshakes, `authenticate`, unauthenticated HTTP); blunts credential-stuffing / connect floods | 10 rps, burst 20 |
| **Global** | node-wide | coarse aggregate safety net against distributed floods no single identity would trip | 5000 rps, burst 10000 |

### Evasion resistance

The bucket key is derived only from **verified** identity. Transports key on the
`user_id` their socket pinned *after* a successful signature check — never on the
unverified id claimed in a packet — so a spoofed id cannot mint a fresh bucket.
Pre-auth traffic is billed to the peer IP, which is hard to spoof on an
established TLS connection.

## Rejection responses

- **TCP / WebSocket** — response code `8` (distinct from the `0–4` action
  codes), body `{"message":"rate_limited","retryAfterMs":N,"scope":"identity|global"}`.
- **HTTP ingress** — `429 Too Many Requests` with a `Retry-After` header and a
  JSON body `{"ok":false,"error":"rate_limited","scope":…,"retryAfterMs":N}`.

## Configuration

All knobs are environment variables (see `node/sample.env`). Anything unset or
malformed falls back to the default.

| Variable | Meaning | Default |
|---|---|---|
| `RATE_LIMIT_ENABLED` | master on/off (`false`/`0`/`no`/`off` disables) | `true` |
| `RATE_LIMIT_AUTH_RPS` | authenticated sustained req/s | `50` |
| `RATE_LIMIT_AUTH_BURST` | authenticated burst capacity | `100` |
| `RATE_LIMIT_ANON_RPS` | anonymous sustained req/s | `10` |
| `RATE_LIMIT_ANON_BURST` | anonymous burst capacity | `20` |
| `RATE_LIMIT_GLOBAL_RPS` | node-wide req/s (`<= 0` disables the global gate) | `5000` |
| `RATE_LIMIT_GLOBAL_BURST` | node-wide burst capacity | `10000` |
| `RATE_LIMIT_IDLE_EVICT_SECS` | evict per-identity buckets idle at least this long | `300` |

When disabled, `check()` always admits the request.

## Memory

Per-identity buckets live in a `DashMap`; a background sweeper
(`ratelimit-sweeper` thread) evicts buckets idle beyond
`RATE_LIMIT_IDLE_EVICT_SECS`, so the map cannot grow unbounded under churny or
adversarial key sets. The sweeper stops automatically once the limiter is
dropped.

## Telemetry

`IRateLimiter::snapshot()` exposes live counters — tracked identities, total
allowed, and rejections split by identity vs. global scope — for wiring into the
node's telemetry surface.

[`IRateLimiter`]: ../node/src/models/ports/ratelimit.rs
