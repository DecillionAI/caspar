// Caspar node — Rust translation of the Caspar (kasper) Go node.
//
// This crate is the result of a phased, file-by-file translation of the
// original Go sources that previously lived under `node/src/**/*.go`.
//
// Translation phases (each landed as its own commit on
// `claude/epic-hypatia-E9EMH`):
//   Phase 0  — workspace restructuring
//   Phase 1  — foundation: `abstractions` (models + adapter traits), logger
//   Phase 2  — Babble hashgraph + chain node consensus engine
//   Phase 3  — core modules (core, globe, actor, pool)
//   Phase 4  — drivers (file, security, signaler, storage, vmm, network)
//   Phase 5  — shell API + utils + app wiring
//   Phase 6  — final cleanup (remove Go sources, tooling, Dockerfile)

#![allow(dead_code)]
// Re-exports in `mod.rs` files are consumed by later translation phases.
#![allow(unused_imports)]
#![allow(clippy::module_inception)]
#![allow(clippy::type_complexity)]

#[macro_use]
mod golog;

mod abstractions;
mod core;
mod multipart;
mod util;

fn main() {
    println!("caspar-node: Rust workspace bootstrapped (translation in progress)");
}
