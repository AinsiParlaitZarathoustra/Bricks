//! Mailbox — host-side pub/sub bus between sandboxes.
//!
//! - Topic = arbitrary string. By convention each sandbox owns
//!   `sandbox/<sandbox_id>` as its private inbox.
//! - Subscribers get a `broadcast::Receiver` wrapped in `MailboxSubscription`.
//! - `publish` is fire-and-forget; if there are no live subscribers the message
//!   is dropped (matches the tokio broadcast semantics — fine for an at-most-
//!   once message bus).

use crate::error::{Result, VmError};
use crate::types::{MailboxEnvelope, SandboxId};
use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::broadcast;

const DEFAULT_CAPACITY: usize = 256;

#[derive(Clone)]
pub struct Mailbox {
    inner: Arc<MailboxInner>,
}

struct MailboxInner {
    topics: DashMap<String, broadcast::Sender<MailboxEnvelope>>,
    seq: AtomicU64,
    capacity: usize,
}

impl Mailbox {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: Arc::new(MailboxInner {
                topics: DashMap::new(),
                seq: AtomicU64::new(0),
                capacity,
            }),
        }
    }

    fn ensure_topic(&self, topic: &str) -> broadcast::Sender<MailboxEnvelope> {
        if let Some(sender) = self.inner.topics.get(topic) {
            return sender.clone();
        }
        let (tx, _) = broadcast::channel(self.inner.capacity);
        self.inner
            .topics
            .entry(topic.to_string())
            .or_insert(tx)
            .clone()
    }

    /// Publish a JSON payload to a topic. Returns the assigned sequence number.
    pub fn publish(
        &self,
        topic: impl Into<String>,
        from: SandboxId,
        payload: serde_json::Value,
    ) -> Result<u64> {
        let topic = topic.into();
        let sender = self.ensure_topic(&topic);
        let seq = self.inner.seq.fetch_add(1, Ordering::Relaxed);
        let env = MailboxEnvelope {
            topic: topic.clone(),
            from,
            seq,
            sent_at_unix_ms: chrono::Utc::now().timestamp_millis(),
            payload,
        };
        // Drop "no live subscribers" silently — at-most-once delivery.
        let _ = sender.send(env);
        Ok(seq)
    }

    /// Subscribe to a topic. Receives messages published *after* this call.
    pub fn subscribe(&self, topic: impl Into<String>) -> MailboxSubscription {
        let topic = topic.into();
        let sender = self.ensure_topic(&topic);
        MailboxSubscription {
            topic,
            receiver: sender.subscribe(),
        }
    }

    /// List currently-known topics.
    pub fn topics(&self) -> Vec<String> {
        self.inner
            .topics
            .iter()
            .map(|kv| kv.key().clone())
            .collect()
    }
}

impl Default for Mailbox {
    fn default() -> Self {
        Self::new()
    }
}

pub struct MailboxSubscription {
    pub topic: String,
    receiver: broadcast::Receiver<MailboxEnvelope>,
}

impl MailboxSubscription {
    /// Await the next envelope. Returns `Err(VmError::Mailbox)` if the bus
    /// has been dropped or the subscriber has lagged past capacity.
    pub async fn recv(&mut self) -> Result<MailboxEnvelope> {
        match self.receiver.recv().await {
            Ok(env) => Ok(env),
            Err(broadcast::error::RecvError::Closed) => {
                Err(VmError::Mailbox("topic closed".into()))
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                Err(VmError::Mailbox(format!("subscriber lagged by {n}")))
            }
        }
    }

    /// Non-blocking poll. `Ok(Some(_))` if a message is ready,
    /// `Ok(None)` if nothing is pending.
    pub fn try_recv(&mut self) -> Result<Option<MailboxEnvelope>> {
        use broadcast::error::TryRecvError;
        match self.receiver.try_recv() {
            Ok(env) => Ok(Some(env)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Closed) => Err(VmError::Mailbox("topic closed".into())),
            Err(TryRecvError::Lagged(n)) => {
                Err(VmError::Mailbox(format!("subscriber lagged by {n}")))
            }
        }
    }
}
