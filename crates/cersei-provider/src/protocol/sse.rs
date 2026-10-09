//! Server-sent events decoder.
//!
//! Works on bytes, not on per-chunk strings: a multi-byte UTF-8 character (or a
//! `\r\n` pair) split across two network reads must reassemble correctly, so
//! text is decoded only once a whole line is available.
//!
//! Rules, from the [SSE standard] unless noted:
//!
//! * An event is dispatched on a blank line, and only if it has at least one
//!   `data` field. `data:` with an empty value counts (the event's data is
//!   then `""`); a block with only `event:` is dropped, and its name does not
//!   carry over to the next event.
//! * Several `data` lines are joined with `\n`. `\n` and `\r\n` end a line;
//!   comment lines (`:` first) are skipped without being stored.
//! * Bound: what is pending for one event — its data, its name and the line
//!   being read — may not exceed [`MAX_PENDING_BYTES`] (16 MiB). Beyond it,
//!   [`SseDecoder::push`] fails with [`SseOverflow`], the buffers are freed
//!   and the decoder stays failed. The bound is per event, not per stream: a
//!   long stream of small events is fine.
//! * Tolerance kept on purpose (not in the standard, which drops an
//!   incomplete event at end of stream): [`SseDecoder::finish`] still
//!   dispatches a last event that has data but no blank line after it.
//! * Not supported: a lone `\r` as a line ending, and the byte order mark the
//!   standard strips before the first line. No supported provider sends
//!   either.
//!
//! [SSE standard]: https://html.spec.whatwg.org/multipage/server-sent-events.html#event-stream-interpretation

/// Bound on what is pending for one event (data, name and current line).
pub const MAX_PENDING_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

/// One event of the stream is larger than the decoder's bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseOverflow {
    pub limit: usize,
}

impl std::fmt::Display for SseOverflow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "malformed event stream: one server-sent event exceeds {} MiB \
             (its data, name and current line); the response was abandoned",
            self.limit / (1024 * 1024)
        )
    }
}

impl std::error::Error for SseOverflow {}

pub struct SseDecoder {
    /// The line being read (without its `\n`).
    line: Vec<u8>,
    /// Inside a comment line: its bytes are dropped until `\n`.
    in_comment: bool,
    event: Option<String>,
    /// `None`: no `data` field yet in this event.
    data: Option<String>,
    limit: usize,
    failed: bool,
}

impl Default for SseDecoder {
    fn default() -> Self {
        Self::with_limit(MAX_PENDING_BYTES)
    }
}

impl SseDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// A decoder with another bound (tests).
    pub(crate) fn with_limit(limit: usize) -> Self {
        Self {
            line: Vec::new(),
            in_comment: false,
            event: None,
            data: None,
            limit,
            failed: false,
        }
    }

    fn pending(&self) -> usize {
        self.line.len()
            + self.event.as_ref().map_or(0, String::len)
            + self.data.as_ref().map_or(0, String::len)
    }

    fn fail(&mut self) -> SseOverflow {
        self.line = Vec::new();
        self.event = None;
        self.data = None;
        self.in_comment = false;
        self.failed = true;
        SseOverflow { limit: self.limit }
    }

    /// Feed bytes; returns the events completed by them, in order.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<SseEvent>, SseOverflow> {
        if self.failed {
            return Err(SseOverflow { limit: self.limit });
        }
        let mut out = Vec::new();
        let mut rest = bytes;
        while !rest.is_empty() {
            let (segment, ends_line) = match rest.iter().position(|b| *b == b'\n') {
                Some(p) => (&rest[..p], true),
                None => (rest, false),
            };
            rest = if ends_line {
                &rest[segment.len() + 1..]
            } else {
                &[]
            };
            if self.in_comment {
                self.in_comment = !ends_line;
                continue;
            }
            if self.line.is_empty() && segment.first() == Some(&b':') {
                self.in_comment = !ends_line;
                continue;
            }
            // Checked before copying: nothing beyond the bound is stored.
            if self.pending() + segment.len() > self.limit {
                return Err(self.fail());
            }
            self.line.extend_from_slice(segment);
            if ends_line {
                let mut line = std::mem::take(&mut self.line);
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                self.on_line(&line, &mut out);
            }
        }
        Ok(out)
    }

    /// End of stream: dispatch a last event that has data even without a
    /// final blank line (see the module documentation).
    pub fn finish(&mut self) -> Vec<SseEvent> {
        let mut out = Vec::new();
        if self.failed {
            return out;
        }
        if !self.line.is_empty() {
            let mut line = std::mem::take(&mut self.line);
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            self.on_line(&line, &mut out);
        }
        self.dispatch(&mut out);
        out
    }

    fn on_line(&mut self, line: &[u8], out: &mut Vec<SseEvent>) {
        if line.is_empty() {
            self.dispatch(out);
            return;
        }
        let text = String::from_utf8_lossy(line);
        let (field, value) = match text.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (text.as_ref(), ""),
        };
        match field {
            "event" => self.event = Some(value.to_string()),
            "data" => match &mut self.data {
                Some(d) => {
                    d.push('\n');
                    d.push_str(value);
                }
                None => self.data = Some(value.to_string()),
            },
            _ => {} // id, retry, unknown
        }
    }

    fn dispatch(&mut self, out: &mut Vec<SseEvent>) {
        let event = self.event.take();
        if let Some(data) = self.data.take() {
            out.push(SseEvent { event, data });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl SseDecoder {
        fn push_ok(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
            self.push(bytes).expect("within the bound")
        }
    }

    #[test]
    fn splits_events_and_names() {
        let mut d = SseDecoder::new();
        let ev = d.push_ok(b"event: a\ndata: {\"x\":1}\n\ndata: second\n\n");
        assert_eq!(ev.len(), 2);
        assert_eq!(
            ev[0],
            SseEvent {
                event: Some("a".into()),
                data: "{\"x\":1}".into()
            }
        );
        assert_eq!(
            ev[1],
            SseEvent {
                event: None,
                data: "second".into()
            }
        );
    }

    #[test]
    fn crlf_and_multiline_data() {
        let mut d = SseDecoder::new();
        let ev = d.push_ok(b"data: one\r\ndata: two\r\n\r\n");
        assert_eq!(ev[0].data, "one\ntwo");
    }

    #[test]
    fn utf8_split_across_chunks_is_reassembled() {
        let full = "data: {\"t\":\"héllo 🌍 世界\"}\n\n".as_bytes().to_vec();
        // Split in the middle of the emoji (4 bytes) and of "é" (2 bytes).
        for cut in 1..full.len() {
            let mut d = SseDecoder::new();
            let mut ev = d.push_ok(&full[..cut]);
            ev.extend(d.push_ok(&full[cut..]));
            assert_eq!(ev.len(), 1, "cut at {cut}");
            assert_eq!(ev[0].data, "{\"t\":\"héllo 🌍 世界\"}", "cut at {cut}");
        }
    }

    #[test]
    fn byte_by_byte_feed() {
        let mut d = SseDecoder::new();
        let mut ev = Vec::new();
        for b in "event: e\ndata: déjà\n\n".bytes() {
            ev.extend(d.push_ok(&[b]));
        }
        assert_eq!(
            ev,
            vec![SseEvent {
                event: Some("e".into()),
                data: "déjà".into()
            }]
        );
    }

    #[test]
    fn comments_ignored_and_eof_flushes() {
        let mut d = SseDecoder::new();
        assert!(d.push_ok(b": ping\n").is_empty());
        assert!(d.push_ok(b"data: tail").is_empty());
        let ev = d.finish();
        assert_eq!(ev[0].data, "tail");
    }

    #[test]
    fn a_name_without_data_is_not_an_event_and_does_not_leak() {
        let mut d = SseDecoder::new();
        let ev = d.push_ok(b"event: ping\n\ndata: x\n\n");
        assert_eq!(
            ev,
            vec![SseEvent {
                event: None,
                data: "x".into()
            }]
        );
        assert!(d.push_ok(b"event: lonely\n").is_empty());
        assert!(d.finish().is_empty());
    }

    #[test]
    fn empty_data_is_an_event() {
        let mut d = SseDecoder::new();
        let ev = d.push_ok(b"event: e\ndata:\n\ndata\n\ndata: \n\n");
        assert_eq!(ev.len(), 3);
        assert_eq!(ev[0].event.as_deref(), Some("e"));
        assert!(ev.iter().all(|e| e.data.is_empty()));
        assert_eq!(ev[1].event, None);
    }

    #[test]
    fn exact_bound_is_accepted_and_one_more_byte_is_refused() {
        // "data: " + payload = the line; pending = line bytes before dispatch.
        let limit = 64;
        let line = format!("data: {}", "x".repeat(limit - 6));
        assert_eq!(line.len(), limit);
        let mut d = SseDecoder::with_limit(limit);
        let ev = d.push_ok(format!("{line}\n\n").as_bytes());
        assert_eq!(ev[0].data.len(), limit - 6);

        let mut d = SseDecoder::with_limit(limit);
        let err = d.push(format!("{line}x\n\n").as_bytes()).unwrap_err();
        assert_eq!(err.limit, limit);
        assert!(err.to_string().contains("exceeds"));
        // Failed for good, buffers freed.
        assert!(d.line.is_empty() && d.data.is_none());
        assert!(d.push(b"data: ok\n\n").is_err());
        assert!(d.finish().is_empty());
    }

    #[test]
    fn a_long_line_is_refused_before_its_end() {
        let mut d = SseDecoder::with_limit(100);
        assert!(d.push_ok(b"data: ").is_empty());
        // No newline ever comes: the bound still applies to the partial line.
        let err = (0..100).find_map(|_| d.push(b"yyyyyyyyyy").err());
        assert!(err.is_some());
    }

    #[test]
    fn many_small_data_lines_add_up() {
        let mut d = SseDecoder::with_limit(100);
        let mut result = Ok(Vec::new());
        for _ in 0..30 {
            result = d.push(b"data: abcd\n");
            if result.is_err() {
                break;
            }
        }
        assert!(result.is_err(), "the accumulated event must hit the bound");
    }

    #[test]
    fn the_name_counts_too() {
        let mut d = SseDecoder::with_limit(32);
        assert!(d
            .push(format!("event: {}\n", "n".repeat(20)).as_bytes())
            .is_ok());
        assert!(d.push(b"data: 0123456789abcdef\n").is_err());
    }

    #[test]
    fn long_comments_are_not_stored() {
        let mut d = SseDecoder::with_limit(16);
        assert!(d.push_ok(b": ").is_empty());
        for _ in 0..1000 {
            assert!(d.push_ok(b"keep-alive padding ").is_empty());
        }
        assert_eq!(d.line.len(), 0);
        let ev = d.push_ok(b"\ndata: ok\n\n");
        assert_eq!(ev[0].data, "ok");
    }

    #[test]
    fn one_chunk_with_many_events_is_not_bounded_as_a_whole() {
        let mut d = SseDecoder::with_limit(64);
        let mut chunk = String::new();
        for i in 0..1000 {
            chunk.push_str(&format!("event: e\ndata: {i}\n\n"));
        }
        assert!(chunk.len() > 64 * 100);
        let ev = d.push_ok(chunk.as_bytes());
        assert_eq!(ev.len(), 1000);
        assert_eq!(ev[999].data, "999");
        assert!(ev.iter().all(|e| e.event.as_deref() == Some("e")));
    }

    #[test]
    fn eof_without_blank_line_keeps_its_data_but_not_a_lone_name() {
        let mut d = SseDecoder::new();
        assert!(d.push_ok(b"data: a\ndata: b").is_empty());
        assert_eq!(d.finish()[0].data, "a\nb");
        let mut d = SseDecoder::new();
        assert!(d.push_ok(b"event: only").is_empty());
        assert!(d.finish().is_empty());
    }
}
