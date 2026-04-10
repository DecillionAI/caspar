# SDK Runtime Samples 🧪

> Updated: **2026-04-10**

Real-world-compatible runtime samples for all supported VM types.

## Included Runtimes

- `wasm/`
  - Uses the AppEngine contract (`malloc`, `run`, and `env.hostCall`)
  - Covers host helpers used across machines + VMM routes (HTTP, trigger, signal, vm/docker control, db ops, sync tasks, `submitOnchainTrx`, chain messaging, token checks, locks)
- `docker/`
  - Mirrors machine/docker TCP packet transport
  - Includes wrappers for core VMM callback keys used by managed runtimes
- `javascript/`
  - Demonstrates QuickJS host bridge usage
  - Includes wrappers for db/lock/sync + VMM callbacks
- `elpify/`
  - `module.masm`: practical MASM billing use case
  - `module.elpify.js`: elpify-language source intended for transpiling via `elpify-lang` in appengine
- `elpian/`
  - Logic-oriented AST sample (`main(order)`)

## Notes 📌

- Keep samples close to host callback contracts used in `node/appengine`.
- Prefer additive examples over breaking changes so integrators can compare versions safely.
