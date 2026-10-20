//! Host-mediated cross-sandbox primitives: Volume, Mailbox, KvStore.

pub mod kv;
pub mod mailbox;
pub mod volume;

pub use kv::{KvSnapshot, KvStore};
pub use mailbox::{Mailbox, MailboxSubscription};
pub use volume::{Volume, VolumeRegistry};
