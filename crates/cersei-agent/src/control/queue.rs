//! Delivery of events to one consumer, bounded, without silent loss.
//!
//! * A text or reasoning delta is merged into the delta queued just before
//!   it (same kind, same run): a slow consumer receives fewer, larger
//!   deltas, with every character.
//! * Any other event waits for room when `capacity` events are queued:
//!   the producer — and so the run — slows down to the consumer's pace.
//!   (A delta never waits: merged with its neighbour, it adds at most one
//!   entry per other event, so the queue stays under `2 × capacity + 1`.)
//! * The wait never holds a cancellation back: once the run's token is
//!   cancelled, events are queued beyond the bound (the run is ending).
//! * Sequence numbers are assigned when an event is taken, so they are
//!   contiguous in what the consumer sees.
//! * When the consumer goes away (`close`), producers stop waiting and
//!   their events are dropped: nobody is left to read them.

use super::protocol::{Envelope, Event, SCHEMA_VERSION};
use std::collections::VecDeque;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

struct Item {
    run_id: Option<String>,
    at: i64,
    event: Event,
}

struct State {
    items: VecDeque<Item>,
    closed: bool,
    seq: u64,
    /// Deltas merged into a previous one, for diagnostics.
    merged: u64,
}

pub struct EventQueue {
    session_id: parking_lot::Mutex<String>,
    capacity: usize,
    state: parking_lot::Mutex<State>,
    readable: Notify,
    writable: Notify,
}

impl EventQueue {
    pub fn new(session_id: &str, capacity: usize) -> Self {
        Self {
            session_id: parking_lot::Mutex::new(session_id.to_string()),
            capacity: capacity.max(1),
            state: parking_lot::Mutex::new(State {
                items: VecDeque::new(),
                closed: false,
                seq: 0,
                merged: 0,
            }),
            readable: Notify::new(),
            writable: Notify::new(),
        }
    }

    /// The session later envelopes carry (after a resume).
    pub fn set_session(&self, session_id: &str) {
        *self.session_id.lock() = session_id.to_string();
    }

    /// Queue an event, waiting for room if needed (see the module docs).
    pub async fn push(
        &self,
        run_id: Option<&str>,
        event: Event,
        cancel: Option<&CancellationToken>,
    ) {
        let at = chrono::Utc::now().timestamp_millis();
        let mut item = Some(Item {
            run_id: run_id.map(str::to_string),
            at,
            event,
        });
        loop {
            let waiter = self.writable.notified();
            tokio::pin!(waiter);
            {
                let mut st = self.state.lock();
                if st.closed {
                    return;
                }
                let it = item.take().expect("pushed once");
                if it.event.is_delta() {
                    if let Some(last) = st.items.back_mut() {
                        if last.run_id == it.run_id && merge(&mut last.event, &it.event) {
                            st.merged += 1;
                            return;
                        }
                    }
                }
                let over = st.items.len() >= self.capacity;
                let unbounded = cancel.is_some_and(|c| c.is_cancelled()) || it.event.is_delta();
                if !over || unbounded {
                    st.items.push_back(it);
                    drop(st);
                    self.readable.notify_one();
                    return;
                }
                item = Some(it);
                waiter.as_mut().enable();
            }
            match cancel {
                Some(c) => {
                    tokio::select! {
                        _ = &mut waiter => {}
                        _ = c.cancelled() => {}
                    }
                }
                None => waiter.await,
            }
        }
    }

    /// The next envelope; `None` once closed and drained.
    pub async fn recv(&self) -> Option<Envelope> {
        loop {
            let waiter = self.readable.notified();
            tokio::pin!(waiter);
            {
                let mut st = self.state.lock();
                if let Some(it) = st.items.pop_front() {
                    st.seq += 1;
                    let seq = st.seq;
                    drop(st);
                    self.writable.notify_one();
                    return Some(Envelope {
                        schema: SCHEMA_VERSION,
                        session_id: self.session_id.lock().clone(),
                        run_id: it.run_id,
                        seq,
                        at: it.at,
                        event: it.event,
                    });
                }
                if st.closed {
                    return None;
                }
                waiter.as_mut().enable();
            }
            waiter.await;
        }
    }

    /// The next envelope if one is queued.
    pub fn try_recv(&self) -> Option<Envelope> {
        let mut st = self.state.lock();
        let it = st.items.pop_front()?;
        st.seq += 1;
        let seq = st.seq;
        drop(st);
        self.writable.notify_one();
        Some(Envelope {
            schema: SCHEMA_VERSION,
            session_id: self.session_id.lock().clone(),
            run_id: it.run_id,
            seq,
            at: it.at,
            event: it.event,
        })
    }

    /// No more events will be read (the consumer stopped) or written (the
    /// session ended): wake everyone. Queued events can still be read.
    pub fn close(&self) {
        self.state.lock().closed = true;
        self.readable.notify_waiters();
        self.writable.notify_waiters();
    }

    pub fn len(&self) -> usize {
        self.state.lock().items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Deltas merged so far.
    pub fn merged(&self) -> u64 {
        self.state.lock().merged
    }
}

fn merge(into: &mut Event, next: &Event) -> bool {
    match (into, next) {
        (Event::TextDelta { text: a }, Event::TextDelta { text: b })
        | (Event::ThinkingDelta { text: a }, Event::ThinkingDelta { text: b }) => {
            a.push_str(b);
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    fn notice(n: usize) -> Event {
        Event::Notice {
            message: format!("n{n}"),
        }
    }

    #[tokio::test]
    async fn deltas_merge_and_sequence_numbers_stay_contiguous() {
        let q = EventQueue::new("s", 4);
        for c in ["a", "b", "c"] {
            q.push(Some("r"), Event::TextDelta { text: c.into() }, None)
                .await;
        }
        q.push(Some("r"), notice(1), None).await;
        q.push(Some("r"), Event::TextDelta { text: "d".into() }, None)
            .await;
        q.push(Some("r"), Event::ThinkingDelta { text: "t".into() }, None)
            .await;
        let mut got = Vec::new();
        while let Some(e) = q.try_recv() {
            got.push(e);
        }
        let seqs: Vec<u64> = got.iter().map(|e| e.seq).collect();
        assert_eq!(seqs, vec![1, 2, 3, 4]);
        assert_eq!(got[0].event, Event::TextDelta { text: "abc".into() });
        assert_eq!(
            got[2].event,
            Event::TextDelta { text: "d".into() },
            "not merged across a notice"
        );
        assert_eq!(q.merged(), 2);
    }

    /// A slow consumer slows the producer down; nothing critical is lost
    /// and every character of the deltas arrives.
    #[tokio::test]
    async fn a_slow_consumer_loses_nothing() {
        let q = Arc::new(EventQueue::new("s", 8));
        let producer = {
            let q = q.clone();
            tokio::spawn(async move {
                for i in 0..200 {
                    q.push(
                        Some("r"),
                        Event::TextDelta {
                            text: format!("{i},"),
                        },
                        None,
                    )
                    .await;
                    q.push(Some("r"), notice(i), None).await;
                }
                q.close();
            })
        };
        let mut text = String::new();
        let mut notices = 0;
        let mut last = 0;
        while let Some(e) = q.recv().await {
            assert_eq!(e.seq, last + 1);
            last = e.seq;
            match e.event {
                Event::TextDelta { text: t } => text.push_str(&t),
                Event::Notice { .. } => notices += 1,
                _ => {}
            }
            // Bounded: at most `capacity` other events, and one merged
            // delta between two of them.
            assert!(q.len() <= 2 * 8 + 1, "bounded: {}", q.len());
            tokio::time::sleep(Duration::from_micros(200)).await;
        }
        producer.await.unwrap();
        assert_eq!(notices, 200);
        let expected: String = (0..200).map(|i| format!("{i},")).collect();
        assert_eq!(text, expected);
    }

    /// With nobody reading, a full queue blocks the producer — until the
    /// run is cancelled: cancellation is never held back by the consumer.
    #[tokio::test]
    async fn cancellation_is_never_blocked_by_a_full_queue() {
        let q = Arc::new(EventQueue::new("s", 2));
        let cancel = CancellationToken::new();
        q.push(None, notice(0), Some(&cancel)).await;
        q.push(None, notice(1), Some(&cancel)).await;
        let blocked = {
            let (q, c) = (q.clone(), cancel.clone());
            tokio::spawn(async move { q.push(None, notice(2), Some(&c)).await })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!blocked.is_finished(), "waits for room");
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(1), blocked)
            .await
            .expect("released by the cancellation")
            .unwrap();
        assert_eq!(q.len(), 3, "kept, beyond the bound");
        q.close();
        q.push(None, notice(3), None).await;
        assert_eq!(q.len(), 3, "dropped once closed");
    }
}
