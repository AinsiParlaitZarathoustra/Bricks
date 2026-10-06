//! Time source of the memory, injectable for tests.

use super::model::Millis;
use std::sync::atomic::{AtomicI64, Ordering};

pub trait Clock: Send + Sync {
    /// Now, in milliseconds since the Unix epoch (UTC).
    fn now(&self) -> Millis;
}

/// The system clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Millis {
        chrono::Utc::now().timestamp_millis()
    }
}

/// A clock that only moves when told to.
#[derive(Debug, Default)]
pub struct ManualClock(AtomicI64);

impl ManualClock {
    pub fn new(at: Millis) -> Self {
        Self(AtomicI64::new(at))
    }

    pub fn set(&self, at: Millis) {
        self.0.store(at, Ordering::SeqCst);
    }

    pub fn advance(&self, by: Millis) {
        self.0.fetch_add(by, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Millis {
        self.0.load(Ordering::SeqCst)
    }
}

/// `2026-03-01`, `2026-03-01T10:00:00Z` or RFC 3339 → milliseconds (UTC).
pub fn parse_time(s: &str) -> Option<Millis> {
    let s = s.trim();
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(t.timestamp_millis());
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Some(d.and_hms_opt(0, 0, 0)?.and_utc().timestamp_millis());
    }
    if let Ok(t) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        return Some(t.and_utc().timestamp_millis());
    }
    // LongMemEval-style `2023/05/20 (Sat) 02:21`.
    let head: String = s.chars().take(10).collect();
    if let Ok(d) = chrono::NaiveDate::parse_from_str(&head, "%Y/%m/%d") {
        let time = s.rsplit(' ').next().unwrap_or("");
        let (h, m) = time
            .split_once(':')
            .and_then(|(h, m)| Some((h.parse().ok()?, m.parse().ok()?)))
            .unwrap_or((0, 0));
        return Some(d.and_hms_opt(h, m, 0)?.and_utc().timestamp_millis());
    }
    None
}

/// Milliseconds → `2026-03-01` (UTC) for display.
pub fn date(ms: Millis) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| ms.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_parse() {
        let d = parse_time("2026-03-01").unwrap();
        assert_eq!(date(d), "2026-03-01");
        assert_eq!(parse_time("2026-03-01T00:00:00Z"), Some(d));
        assert_eq!(
            date(parse_time("2023/05/20 (Sat) 02:21").unwrap()),
            "2023-05-20"
        );
        assert_eq!(parse_time("yesterday"), None);
        let c = ManualClock::new(5);
        c.advance(10);
        assert_eq!(c.now(), 15);
    }
}
