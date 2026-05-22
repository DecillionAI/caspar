//! Translation of `shell/api/model` — the storage-backed entity models the
//! Caspar shell uses. Each model carries a `type()` discriminator, a `push`
//! that writes its columns/indices through the `ITrx`, and a `pull` /
//! `all` / `list` family that reads them back.

pub mod chain;
pub mod creature;
pub mod entity;
pub mod file;
pub mod machine_program;
pub mod session;
pub mod store;
pub mod user;

pub use chain::{Chain, ChainShard};
pub use creature::Creature;
pub use entity::Entity;
pub use file::File;
pub use machine_program::{Machine, Program};
pub use session::Session;
pub use store::Store;
pub use user::User;
