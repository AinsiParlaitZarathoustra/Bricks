//! Fan-out of desktop events to the subscribed windows.
//!
//! Each subscription has a bounded queue drained by its own task, which
//! writes to the channel. Publishing never waits: a subscriber whose queue
//! is full misses the event, and is told so with `Lagged` for that session
//! once it can take it again (the view then reloads the session).

use crate::contract::{ChangeRecord, DesktopEvent};
use crate::services::changes::ChangeJournal;
use std::collections::{HashMap, HashSet};
use tauri::ipc::Channel;
use tokio::sync::mpsc::error::TrySendError;

/// Events a subscriber may have waiting before it starts missing some.
const SUBSCRIBER_QUEUE: usize = 2048;

struct Subscriber {
    tx: tokio::sync::mpsc::Sender<DesktopEvent>,
    /// Sessions whose events were missed: a `Lagged` notice is owed.
    lagged: HashSet<String>,
}

pub struct Hub {
    subscribers: parking_lot::Mutex<HashMap<String, Subscriber>>,
    pub journal: std::sync::Arc<ChangeJournal>,
}

impl Hub {
    pub fn new(journal: std::sync::Arc<ChangeJournal>) -> Self {
        Self {
            subscribers: parking_lot::Mutex::new(HashMap::new()),
            journal,
        }
    }

    /// Subscribes `channel` to every session's events; returns the
    /// subscription id.
    pub fn subscribe(&self, channel: Channel<DesktopEvent>) -> String {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<DesktopEvent>(SUBSCRIBER_QUEUE);
        tauri::async_runtime::spawn(async move {
            while let Some(event) = rx.recv().await {
                if channel.send(event).is_err() {
                    // The window is gone: dropping `rx` ends the subscription.
                    break;
                }
            }
        });
        self.subscribers.lock().insert(
            id.clone(),
            Subscriber {
                tx,
                lagged: HashSet::new(),
            },
        );
        id
    }

    pub fn unsubscribe(&self, subscription_id: &str) -> bool {
        self.subscribers.lock().remove(subscription_id).is_some()
    }

    /// Sends `event` to every subscriber without waiting for any of them.
    pub fn publish(&self, event: DesktopEvent) {
        let session = session_of(&event);
        let mut subs = self.subscribers.lock();
        subs.retain(|_, sub| {
            // Owed notices first, while there is room for them.
            let owed: Vec<String> = sub.lagged.iter().cloned().collect();
            for session_id in owed {
                match sub.tx.try_send(DesktopEvent::Lagged {
                    session_id: session_id.clone(),
                }) {
                    Ok(()) => {
                        sub.lagged.remove(&session_id);
                    }
                    Err(TrySendError::Full(_)) => break,
                    Err(TrySendError::Closed(_)) => return false,
                }
            }
            match sub.tx.try_send(event.clone()) {
                Ok(()) => true,
                Err(TrySendError::Full(_)) => {
                    if let Some(id) = &session {
                        sub.lagged.insert(id.clone());
                    }
                    true
                }
                Err(TrySendError::Closed(_)) => false,
            }
        });
    }

    /// A journal entry, announced to every window.
    pub fn record_added(&self, record: ChangeRecord) {
        self.publish(DesktopEvent::ChangeRecorded { record });
    }
}

/// The session an event is about, when it has one.
fn session_of(event: &DesktopEvent) -> Option<String> {
    match event {
        DesktopEvent::Engine { envelope, .. } => envelope
            .get("session_id")
            .and_then(|v| v.as_str())
            .map(String::from),
        DesktopEvent::SessionState { session_id, .. }
        | DesktopEvent::SessionDeleted { session_id }
        | DesktopEvent::Lagged { session_id } => Some(session_id.clone()),
        DesktopEvent::SessionUpdated { session } => Some(session.id.clone()),
        DesktopEvent::ChangeRecorded { record } => Some(record.session_id.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use tauri::ipc::{Channel, InvokeResponseBody};

    #[tokio::test(flavor = "multi_thread")]
    async fn a_stuck_window_never_blocks_publishing_and_is_told_what_it_missed() {
        let dir = tempfile::tempdir().unwrap();
        let hub = Hub::new(Arc::new(ChangeJournal::at(dir.path().to_path_buf())));
        // The window is stuck until the test releases it.
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let release_rx = Mutex::new(release_rx);
        let (seen_tx, seen_rx) = std::sync::mpsc::channel::<String>();
        let channel: Channel<DesktopEvent> = Channel::new(move |body| {
            let _ = release_rx.lock().unwrap().recv();
            if let InvokeResponseBody::Json(text) = body {
                let _ = seen_tx.send(text);
            }
            Ok(())
        });
        let id = hub.subscribe(channel);

        let started = Instant::now();
        for i in 0..5000 {
            hub.publish(DesktopEvent::SessionState {
                session_id: "s1".into(),
                open: true,
                running: i % 2 == 0,
            });
        }
        assert!(started.elapsed() < Duration::from_secs(5), "publishing never waits");

        // Released, the window takes events again; the missed session is
        // announced on a later publication.
        for _ in 0..10_000 {
            let _ = release_tx.send(());
        }
        let mut lagged = false;
        for _ in 0..200 {
            hub.publish(DesktopEvent::SessionState {
                session_id: "s2".into(),
                open: false,
                running: false,
            });
            while let Ok(text) = seen_rx.try_recv() {
                if text.contains("\"kind\":\"lagged\"") && text.contains("\"s1\"") {
                    lagged = true;
                }
            }
            if lagged {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(lagged, "the missed session is announced");
        assert!(hub.unsubscribe(&id));
        assert!(!hub.unsubscribe(&id), "unknown ids are reported as such");
    }
}
