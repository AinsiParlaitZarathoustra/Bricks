//! Server-sent events decoder.
//!
//! Works on bytes, not on per-chunk strings: a multi-byte UTF-8 character (or a
//! `\r\n` pair) split across two network reads must reassemble correctly, so
//! text is decoded only once a whole line is available.

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

#[derive(Default)]
pub struct SseDecoder {
    buf: Vec<u8>,
    event: Option<String>,
    data: Vec<String>,
}

impl SseDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed bytes; returns the events completed by them.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some(pos) = self.buf.iter().position(|b| *b == b'\n') {
            let mut line: Vec<u8> = self.buf.drain(..=pos).collect();
            line.pop(); // '\n'
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            self.on_line(&line, &mut out);
        }
        out
    }

    /// Flush a final event when the stream ends without a blank line.
    pub fn finish(&mut self) -> Vec<SseEvent> {
        let mut out = Vec::new();
        if !self.buf.is_empty() {
            let mut line = std::mem::take(&mut self.buf);
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
        if line[0] == b':' {
            return; // comment / keep-alive
        }
        let text = String::from_utf8_lossy(line);
        let (field, value) = match text.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (text.as_ref(), ""),
        };
        match field {
            "event" => self.event = Some(value.to_string()),
            "data" => self.data.push(value.to_string()),
            _ => {} // id, retry, unknown
        }
    }

    fn dispatch(&mut self, out: &mut Vec<SseEvent>) {
        if self.data.is_empty() && self.event.is_none() {
            return;
        }
        out.push(SseEvent {
            event: self.event.take(),
            data: std::mem::take(&mut self.data).join("\n"),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_events_and_names() {
        let mut d = SseDecoder::new();
        let ev = d.push(b"event: a\ndata: {\"x\":1}\n\ndata: second\n\n");
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
        let ev = d.push(b"data: one\r\ndata: two\r\n\r\n");
        assert_eq!(ev[0].data, "one\ntwo");
    }

    #[test]
    fn utf8_split_across_chunks_is_reassembled() {
        let full = "data: {\"t\":\"héllo 🌍 世界\"}\n\n".as_bytes().to_vec();
        // Split in the middle of the emoji (4 bytes) and of "é" (2 bytes).
        for cut in 1..full.len() {
            let mut d = SseDecoder::new();
            let mut ev = d.push(&full[..cut]);
            ev.extend(d.push(&full[cut..]));
            assert_eq!(ev.len(), 1, "cut at {cut}");
            assert_eq!(ev[0].data, "{\"t\":\"héllo 🌍 世界\"}", "cut at {cut}");
        }
    }

    #[test]
    fn byte_by_byte_feed() {
        let mut d = SseDecoder::new();
        let mut ev = Vec::new();
        for b in "event: e\ndata: déjà\n\n".bytes() {
            ev.extend(d.push(&[b]));
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
        assert!(d.push(b": ping\n").is_empty());
        assert!(d.push(b"data: tail").is_empty());
        let ev = d.finish();
        assert_eq!(ev[0].data, "tail");
    }
}
