# SDK runtime samples

Real-world-compatible runtime samples for all supported VM types.

- `wasm/` uses the AppEngine contract (`malloc`, `run`, and single `env.hostCall`) and now covers full host helpers used across machines + vmm routes (http, trigger, signal, vm/docker control, db ops, sync tasks, submitOnchainTrx, chain messaging, token checks, locks).
- `docker/` mirrors machines/docker TCP packet transport and includes wrappers for all core vmm callback keys used by managed runtimes.
- `javascript/` shows QuickJS host bridge usage and includes wrappers for db/lock/sync + vmm callback operations.
- `elpify/` includes:
  - `module.masm`: a real MASM billing use case.
  - `module.elpify.js`: an elpify-language source example that is intended to be transpiled to MASM via `elpify-lang` compiler module in appengine.
- `elpian/` provides a real logic-focused AST (`main(order)`), not a UI render sample.

## Notes

- Host helper names were aligned to patterns from the Kasper `machines` folder and mapped to current appengine (`hostCall` op schema) + vmm callback routing (`key`/`input`).
- No API keys or secrets were copied.
