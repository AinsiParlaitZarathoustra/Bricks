//! How durations are shown to people: always in milliseconds.
//!
//! `12 ms`, `12345 ms` (never switched to seconds), `<1 ms` when the
//! measurement shows a positive duration under a millisecond, `0 ms` when
//! it is zero or its precision cannot tell. Only presentation: stored values
//! (`duration_ms`) and configured limits (timeouts in seconds) are not
//! changed.

use std::time::Duration;

/// A measured duration (monotonic clock), in milliseconds.
pub fn display_ms(d: Duration) -> String {
    if d.is_zero() {
        "0 ms".to_string()
    } else if d < Duration::from_millis(1) {
        "<1 ms".to_string()
    } else {
        format!("{} ms", d.as_millis())
    }
}

/// A duration already counted in whole milliseconds: sub-millisecond
/// precision is gone, so a zero stays `0 ms`.
pub fn display_ms_u64(ms: u64) -> String {
    format!("{ms} ms")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn always_milliseconds() {
        assert_eq!(display_ms(Duration::ZERO), "0 ms");
        assert_eq!(display_ms(Duration::from_micros(400)), "<1 ms");
        assert_eq!(display_ms(Duration::from_millis(100)), "100 ms");
        assert_eq!(display_ms(Duration::from_micros(1_999)), "1 ms");
        assert_eq!(display_ms(Duration::from_millis(12_345)), "12345 ms");
        assert_eq!(display_ms(Duration::from_secs(3_600)), "3600000 ms");
        assert_eq!(display_ms_u64(0), "0 ms");
        assert_eq!(display_ms_u64(12_345), "12345 ms");
    }
}
