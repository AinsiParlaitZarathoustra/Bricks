//! The activity slots of sub-agents, with short admission windows: a queued
//! child gets its slot when one is released in time, expires explicitly
//! when not, gives its place back when cancelled, and a child that waits for
//! its own children never keeps the slot it gave up.

use bricks_agent::agents::runtime::{ActivityLease, AdmissionError, Limits, Scheduler};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn limits(max_concurrent: usize, max_queued: usize, window: Duration) -> Limits {
    Limits {
        max_concurrent,
        max_depth: 3,
        max_queued,
        admission_timeout: window,
        background_drain: Duration::from_secs(5),
    }
}

#[tokio::test]
async fn a_queued_child_gets_the_slot_released_inside_its_window() {
    let s = Scheduler::new(limits(1, 4, Duration::from_millis(500)));
    let never = CancellationToken::new();
    let holder = s.acquire(&never, || {}).await.unwrap();
    let s2 = Arc::clone(&s);
    let waiter = tokio::spawn(async move {
        let mut queued = false;
        let p = s2
            .acquire(&CancellationToken::new(), || queued = true)
            .await;
        (p, queued)
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(s.stats().queued, 1, "one child waits for the slot");
    drop(holder);
    let (got, queued) = waiter.await.unwrap();
    assert!(queued && got.is_ok(), "{got:?}");
    assert_eq!(s.stats().queued, 0, "the place in the queue is given back");
    assert_eq!(s.stats().active, 1, "the waiter holds the slot it got");
    drop(got);
    assert_eq!(s.stats().active, 0);
}

#[tokio::test]
async fn an_expired_window_leaves_nothing_reserved() {
    let s = Scheduler::new(limits(1, 4, Duration::from_millis(60)));
    let never = CancellationToken::new();
    let holder = s.acquire(&never, || {}).await.unwrap();
    let started = std::time::Instant::now();
    let r = s.acquire(&never, || {}).await;
    assert_eq!(r.err(), Some(AdmissionError::Timeout { ms: 60 }));
    assert!(started.elapsed() >= Duration::from_millis(55));
    assert_eq!(s.stats().queued, 0);
    assert_eq!(s.stats().active, 1, "only the holder");
    drop(holder);
    assert_eq!(s.stats().active, 0);
    assert!(
        s.acquire(&never, || {}).await.is_ok(),
        "the slot is usable again"
    );
}

#[tokio::test]
async fn a_cancelled_queued_child_gives_its_place_back() {
    let s = Scheduler::new(limits(1, 4, Duration::from_secs(30)));
    let never = CancellationToken::new();
    let holder = s.acquire(&never, || {}).await.unwrap();
    let cancel = CancellationToken::new();
    let s2 = Arc::clone(&s);
    let c2 = cancel.clone();
    let waiter = tokio::spawn(async move { s2.acquire(&c2, || {}).await.err() });
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(s.stats().queued, 1);
    cancel.cancel();
    assert_eq!(waiter.await.unwrap(), Some(AdmissionError::Cancelled));
    assert_eq!(s.stats().queued, 0);
    drop(holder);
    assert_eq!(
        s.stats().active,
        0,
        "nothing was taken for the cancelled child"
    );
}

#[tokio::test]
async fn a_child_waiting_for_its_own_children_hands_its_slot_over_and_takes_it_back() {
    let s = Scheduler::new(limits(1, 4, Duration::from_millis(500)));
    let never = CancellationToken::new();
    let slot = s.acquire(&never, || {}).await.unwrap();
    let lease = ActivityLease::new(Arc::clone(&s), "parent".into(), slot, None, never.clone());
    let s2 = Arc::clone(&s);
    let got = lease
        .waiting(async move {
            // The grandchild admitted in the freed slot, then released.
            let p = s2.acquire(&CancellationToken::new(), || {}).await.ok();
            drop(p);
            true
        })
        .await;
    assert!(got);
    assert_eq!(s.stats().active, 1, "the parent holds its slot again");
    lease.release().await;
    assert_eq!(s.stats().active, 0);
}

/// BUG: the second of two waits of one child can wait forever for the slot
/// the first one takes back. Interleaving: the first wait's `fetch_sub`
/// reaches 0 and it starts reacquiring; a second wait then sees 0 too, finds
/// nothing to take (the slot is not set yet), and ends its own wait; its
/// reacquire then needs a slot that the first one has just set on the lease,
/// with `max_concurrent = 1`: nobody releases it. About 1 round in 800 with
/// `yield_now` in the waits, on the machine where this was found.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_concurrent_waits_of_one_child_never_deadlock_on_its_own_slot() {
    for round in 0..20_000 {
        let s = Scheduler::new(limits(1, 4, Duration::from_secs(5)));
        let never = CancellationToken::new();
        let slot = s.acquire(&never, || {}).await.unwrap();
        let lease = ActivityLease::new(Arc::clone(&s), "child".into(), slot, None, never.clone());
        let mut a = {
            let l = Arc::clone(&lease);
            tokio::spawn(async move { l.waiting(async { tokio::task::yield_now().await }).await })
        };
        let mut b = {
            let l = Arc::clone(&lease);
            tokio::spawn(async move { l.waiting(async { tokio::task::yield_now().await }).await })
        };
        let both = tokio::time::timeout(Duration::from_millis(500), async {
            (&mut a).await.unwrap();
            (&mut b).await.unwrap();
        })
        .await;
        if both.is_err() {
            let active = s.stats().active;
            never.cancel();
            panic!(
                "round {round}: a second wait is stuck, {active} slot(s) active (the child's own)"
            );
        }
        lease.release().await;
        assert_eq!(s.stats().active, 0, "round {round}: a slot leaked");
    }
}

/// The windows and the drain as configured: 60 s admission, 16 slots, a
/// drain that waits for sub-agents still running (they now always run in
/// parallel), and an explicit value read as given. Shown with the config
/// reader, not by waiting.
#[test]
fn the_defaults_are_16_slots_60_seconds_of_admission_and_a_waiting_drain() {
    let d = bricks_agent::BricksConfig::from_texts(None, &[]);
    assert_eq!(d.agents.admission_timeout_ms, 60_000);
    assert_eq!(d.agents.background_drain_ms, 600_000);
    assert_eq!(d.agents.max_concurrent, 16);
    assert_eq!(d.agents.summary_chars, 30_000);
    assert_eq!(d.agents.max_depth, 1);
    let explicit = bricks_agent::BricksConfig::from_texts(
        Some("[agents]\nadmission_timeout_ms = 1500\nbackground_drain_ms = 600000\n"),
        &[],
    );
    assert_eq!(explicit.agents.admission_timeout_ms, 1500);
    assert_eq!(explicit.agents.background_drain_ms, 600_000);
}
