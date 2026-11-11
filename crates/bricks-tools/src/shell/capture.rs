//! Bounded capture of one output stream.
//!
//! A stream is read from the moment the command starts, chunk by chunk. The
//! raw bytes are kept in memory up to a limit and then spilled to a file, so
//! the complete original stays readable. The cleaned view keeps a head and a
//! tail within fixed sizes; what lies between them is counted and the gap is
//! stated in the text, with the path of the raw file.
//!
//! The capture lives behind a mutex shared with the reader task, so bytes
//! already read are never lost when the waiting future is cancelled.

use super::clean::TerminalCleaner;
use std::io::Write;
use std::path::PathBuf;

/// Size limits of a capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureLimits {
    /// Cleaned text kept from the start.
    pub head_bytes: usize,
    /// Cleaned text kept from the end.
    pub tail_bytes: usize,
    /// Raw bytes kept in memory before spilling to a file.
    pub raw_memory_bytes: usize,
}

impl Default for CaptureLimits {
    fn default() -> Self {
        Self {
            head_bytes: 1024 * 1024,
            tail_bytes: 1024 * 1024,
            raw_memory_bytes: 4 * 1024 * 1024,
        }
    }
}

#[derive(Debug)]
pub struct StreamCapture {
    limits: CaptureLimits,
    cleaner: TerminalCleaner,
    head: String,
    tail: String,
    omitted_bytes: u64,
    raw_bytes: u64,
    raw_memory: Vec<u8>,
    spill_path: PathBuf,
    spill: Option<std::fs::File>,
    spill_error: Option<String>,
}

/// The finished capture.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Captured {
    /// Cleaned text; when the middle was dropped, the gap is stated in it.
    pub text: String,
    /// Bytes received on the stream.
    pub raw_bytes: u64,
    /// Cleaned bytes dropped between head and tail.
    pub omitted_bytes: u64,
    /// Where the complete raw stream was written, when it did not fit in
    /// memory.
    pub raw_path: Option<PathBuf>,
}

impl Captured {
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

impl StreamCapture {
    /// `spill_path` is used only if the raw stream outgrows memory.
    pub fn new(limits: CaptureLimits, spill_path: PathBuf) -> Self {
        Self {
            limits,
            cleaner: TerminalCleaner::new(),
            head: String::new(),
            tail: String::new(),
            omitted_bytes: 0,
            raw_bytes: 0,
            raw_memory: Vec::new(),
            spill_path,
            spill: None,
            spill_error: None,
        }
    }

    pub fn raw_bytes(&self) -> u64 {
        self.raw_bytes
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.raw_bytes += bytes.len() as u64;
        self.keep_raw(bytes);
        let text = self.cleaner.push(bytes);
        self.add_text(&text);
    }

    fn keep_raw(&mut self, bytes: &[u8]) {
        if let Some(f) = &mut self.spill {
            if let Err(e) = f.write_all(bytes) {
                self.spill_error = Some(e.to_string());
                self.spill = None;
            }
            return;
        }
        if self.spill_error.is_some() {
            return;
        }
        if self.raw_memory.len() + bytes.len() <= self.limits.raw_memory_bytes {
            self.raw_memory.extend_from_slice(bytes);
            return;
        }
        // Outgrown memory: everything so far, then the rest, goes to disk.
        let opened = self
            .spill_path
            .parent()
            .map(std::fs::create_dir_all)
            .transpose()
            .and_then(|_| std::fs::File::create(&self.spill_path));
        match opened {
            Ok(mut f) => {
                let res = f
                    .write_all(&self.raw_memory)
                    .and_then(|_| f.write_all(bytes));
                self.raw_memory = Vec::new();
                match res {
                    Ok(()) => self.spill = Some(f),
                    Err(e) => self.spill_error = Some(e.to_string()),
                }
            }
            Err(e) => {
                self.raw_memory = Vec::new();
                self.spill_error = Some(e.to_string());
            }
        }
    }

    fn add_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let room = self.limits.head_bytes.saturating_sub(self.head.len());
        // The head is the start of the stream: once the tail took something,
        // even if it was since dropped, the head is closed.
        let head_open = self.tail.is_empty() && self.omitted_bytes == 0;
        if head_open && text.len() <= room {
            self.head.push_str(text);
            return;
        }
        let mut rest = text;
        if head_open && room > 0 {
            let cut = floor_boundary(text, room);
            self.head.push_str(&text[..cut]);
            rest = &text[cut..];
        }
        self.tail.push_str(rest);
        // Compacted by halves while the stream goes on: the exact bound is
        // applied by `view` and `finish`.
        if self.tail.len() > self.limits.tail_bytes * 2 {
            self.compact_tail();
        }
    }

    /// Keeps at most `tail_bytes` of the tail, counting what goes as omitted.
    fn compact_tail(&mut self) {
        let drop = tail_excess(&self.tail, self.limits.tail_bytes);
        if drop > 0 {
            self.omitted_bytes += drop as u64;
            self.tail.drain(..drop);
        }
    }

    /// Drops the capture and the raw file it spilled to, if any. Nothing
    /// reads them afterwards: used once a command was handed over as a job.
    pub fn discard(mut self) {
        self.spill = None;
        self.raw_memory = Vec::new();
        let _ = std::fs::remove_file(&self.spill_path);
    }

    pub fn finish(mut self) -> Captured {
        let rest = self.cleaner.finish();
        self.add_text(&rest);
        self.compact_tail();
        if let Some(f) = &mut self.spill {
            let _ = f.flush();
        }
        self.view()
    }

    /// The output so far, the capture going on (a partial last line is not
    /// in it, it is not finished yet).
    pub fn view(&self) -> Captured {
        let raw_path = self.spill.as_ref().map(|_| self.spill_path.clone());
        // The tail within its bound, whatever the compaction left.
        let drop = tail_excess(&self.tail, self.limits.tail_bytes);
        let omitted_bytes = self.omitted_bytes + drop as u64;
        let mut text = self.head.clone();
        if omitted_bytes > 0 {
            if !text.ends_with('\n') {
                text.push('\n');
            }
            let where_ = match (&raw_path, &self.spill_error) {
                (Some(p), _) => format!("complete raw output: {}", p.display()),
                (None, Some(e)) => format!("the raw output could not be saved: {e}"),
                (None, None) => "the raw output was not saved".to_string(),
            };
            text.push_str(&format!(
                "[… {omitted_bytes} bytes of output omitted here; {where_}]\n"
            ));
        }
        text.push_str(&self.tail[drop..]);
        Captured {
            text,
            raw_bytes: self.raw_bytes,
            omitted_bytes,
            raw_path,
        }
    }
}

/// The bytes to drop at the start of `tail` so that at most `keep` remain:
/// cut on a character boundary, and, when there is one, just after a line end.
fn tail_excess(tail: &str, keep: usize) -> usize {
    if tail.len() <= keep {
        return 0;
    }
    // At least the excess goes: rounded up to a character boundary.
    let mut drop = tail.len() - keep;
    while drop < tail.len() && !tail.is_char_boundary(drop) {
        drop += 1;
    }
    // Prefer to restart the tail on a line boundary, if one comes soon enough
    // to keep most of it.
    tail[drop..]
        .find('\n')
        .map(|p| drop + p + 1)
        .filter(|&d| d < tail.len() && d - drop <= keep / 2)
        .unwrap_or(drop)
}

fn floor_boundary(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small(dir: &std::path::Path) -> StreamCapture {
        StreamCapture::new(
            CaptureLimits {
                head_bytes: 20,
                tail_bytes: 20,
                raw_memory_bytes: 64,
            },
            dir.join("raw.out"),
        )
    }

    #[test]
    fn short_output_is_kept_whole() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = small(dir.path());
        c.push(b"hello\n");
        let out = c.finish();
        assert_eq!(out.text, "hello\n");
        assert_eq!(out.raw_path, None);
    }

    #[test]
    fn long_output_keeps_head_and_tail_and_spills_the_raw_stream() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = small(dir.path());
        let mut all = Vec::new();
        for i in 0..100 {
            let line = format!("\x1b[1mline {i}\x1b[0m\n");
            all.extend_from_slice(line.as_bytes());
            c.push(line.as_bytes());
        }
        let out = c.finish();
        assert!(out.text.starts_with("line 0\n"), "{}", out.text);
        assert!(out.text.ends_with("line 99\n"), "{}", out.text);
        assert!(out.text.contains("bytes of output omitted here"));
        assert!(out.omitted_bytes > 0);
        let raw = std::fs::read(out.raw_path.unwrap()).unwrap();
        assert_eq!(
            raw, all,
            "the raw file is the exact stream, escapes included"
        );
    }

    /// The tail of the text, after the omitted marker.
    fn shown_tail(text: &str) -> &str {
        text.split_once("]\n").map_or(text, |(_, t)| t)
    }

    #[test]
    fn the_tail_never_exceeds_its_bound() {
        let dir = tempfile::tempdir().unwrap();
        // 55 bytes, one line: the head takes 20, the tail would take 35.
        let input = "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMNOPQR\n";
        assert_eq!(input.len(), 55);
        let mut c = small(dir.path());
        c.push(input.as_bytes());
        let live = c.view();
        assert_eq!(live.omitted_bytes, 15, "{live:?}");
        assert!(shown_tail(&live.text).len() <= 20, "{}", live.text);
        let out = c.finish();
        assert_eq!(out.omitted_bytes, 15);
        assert!(out.text.starts_with("abcdefghijklmnopqrst"), "{}", out.text);
        assert_eq!(shown_tail(&out.text), &input[35..]);
        let marker = out.text.lines().nth(1).unwrap();
        assert!(marker.contains("15 bytes of output omitted"), "{marker}");
        // The head, the line end that closes it, the marker line, the tail.
        assert_eq!(out.text.len(), 20 + 1 + marker.len() + 1 + 20);
    }

    #[test]
    fn the_tail_is_cut_on_a_character_and_prefers_a_line_end() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = small(dir.path());
        // The head, then "é" (2 bytes) straddling the cut, then two lines.
        c.push("01234567890123456789".as_bytes());
        c.push("ééééé\nlast line\n".as_bytes());
        let out = c.finish();
        let tail = shown_tail(&out.text);
        assert!(tail.len() <= 20, "{tail:?}");
        assert_eq!(tail, "last line\n");
        assert_eq!(out.omitted_bytes as usize, "ééééé\n".len());
    }

    #[test]
    fn the_head_stays_the_start_of_the_stream_when_the_tail_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = StreamCapture::new(
            CaptureLimits {
                head_bytes: 4,
                tail_bytes: 0,
                raw_memory_bytes: 64,
            },
            dir.path().join("raw.out"),
        );
        for piece in ["ab\n", "é\n", "z\n"] {
            c.push(piece.as_bytes());
        }
        let out = c.finish();
        assert!(out.text.starts_with("ab\n["), "{}", out.text);
        assert_eq!(out.omitted_bytes, 5);
    }

    #[test]
    fn discard_removes_the_spilled_raw_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("raw.out");
        let mut c = StreamCapture::new(
            CaptureLimits {
                head_bytes: 20,
                tail_bytes: 20,
                raw_memory_bytes: 64,
            },
            path.clone(),
        );
        for i in 0..100 {
            c.push(format!("line {i}\n").as_bytes());
        }
        assert!(path.exists(), "the raw stream spilled to its file");
        assert!(c.view().raw_path.is_some());
        c.discard();
        assert!(!path.exists(), "discard removes the spilled raw file");
    }
}
