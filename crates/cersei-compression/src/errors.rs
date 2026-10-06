//! Find the diagnostic blocks of a log before anything is cut.
//!
//! Compilation errors, failed tests, panics and tracebacks are what a reader
//! of a long log needs, and they often sit in the middle, where a head/tail
//! cut loses them. Detection runs on the cleaned lines, before rule filtering
//! and truncation; the lines it marks are kept in priority.
//!
//! The built-in detectors cover the formats emitted by rustc/cargo, Go,
//! Python/pytest, Node and the JavaScript test runners, TypeScript, ESLint,
//! Terraform, kubectl, Docker and uv. Rules can add their own detectors.

use once_cell::sync::Lazy;
use regex::Regex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Warning,
    Error,
}

impl Severity {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "error" => Some(Severity::Error),
            "warning" => Some(Severity::Warning),
            _ => None,
        }
    }
}

/// A detected diagnostic block: lines `start..=end` of the cleaned log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub start: usize,
    pub end: usize,
    pub severity: Severity,
    pub kind: String,
}

impl Block {
    pub fn len(&self) -> usize {
        self.end - self.start + 1
    }
    pub fn is_empty(&self) -> bool {
        false
    }
}

/// How a block that starts on a matching line ends.
#[derive(Debug, Clone)]
pub enum BlockEnd {
    /// Only the matching line.
    Single,
    /// Up to (excluding) the next blank line.
    Blank,
    /// Following lines that are indented (or blank lines followed by one).
    Indented,
    /// Up to the first line matching the regex (inclusive or not).
    Until { pattern: Regex, inclusive: bool },
    /// Up to and including the first non-indented line (Python tracebacks).
    FirstUnindented,
    /// Following lines as long as they match the regex.
    While(Regex),
}

#[derive(Debug, Clone)]
pub struct Detector {
    pub kind: String,
    pub start: Regex,
    pub end: BlockEnd,
    pub severity: Severity,
    /// Downgrade to a warning when a line of the block matches.
    pub warning_if: Option<Regex>,
    /// Blocks shorter than this are not blocks (avoids headings).
    pub min_lines: usize,
    /// Scan limit for the block end.
    pub max_lines: usize,
}

impl Detector {
    fn new(kind: &str, start: &str, end: BlockEnd, severity: Severity) -> Self {
        Detector {
            kind: kind.into(),
            start: Regex::new(start).expect("built-in detector regex"),
            end,
            severity,
            warning_if: None,
            min_lines: 1,
            max_lines: 400,
        }
    }

    fn until(pattern: &str, inclusive: bool) -> BlockEnd {
        BlockEnd::Until {
            pattern: Regex::new(pattern).expect("built-in detector regex"),
            inclusive,
        }
    }

    fn while_matching(pattern: &str) -> BlockEnd {
        BlockEnd::While(Regex::new(pattern).expect("built-in detector regex"))
    }

    fn block_end(&self, lines: &[String], start: usize) -> usize {
        let last = lines.len().saturating_sub(1).min(start + self.max_lines);
        match &self.end {
            BlockEnd::Single => start,
            BlockEnd::Blank => {
                let mut i = start;
                while i < last && !lines[i + 1].trim().is_empty() {
                    i += 1;
                }
                i
            }
            BlockEnd::Indented => {
                let mut i = start;
                while i < last {
                    let next = &lines[i + 1];
                    if is_indented(next) {
                        i += 1;
                    } else if next.trim().is_empty() && i + 2 <= last && is_indented(&lines[i + 2])
                    {
                        i += 2;
                    } else {
                        break;
                    }
                }
                i
            }
            BlockEnd::Until { pattern, inclusive } => {
                let mut i = start;
                while i < last {
                    if pattern.is_match(&lines[i + 1]) {
                        return if *inclusive { i + 1 } else { i };
                    }
                    i += 1;
                }
                i
            }
            BlockEnd::While(pattern) => {
                let mut i = start;
                while i < last && pattern.is_match(&lines[i + 1]) {
                    i += 1;
                }
                i
            }
            BlockEnd::FirstUnindented => {
                let mut i = start;
                while i < last {
                    i += 1;
                    let l = &lines[i];
                    if !l.trim().is_empty() && !is_indented(l) {
                        return i;
                    }
                }
                i
            }
        }
    }
}

fn is_indented(l: &str) -> bool {
    l.starts_with(' ') || l.starts_with('\t')
}

const RUST_CONTINUATION: &str = r"^(\s|\d+\s*[|+-]|help:|note:|= )";

static BUILTIN: Lazy<Vec<Detector>> = Lazy::new(|| {
    use BlockEnd::*;
    use Severity::*;
    let mut d = vec![
        // ── Rust / cargo ──
        Detector::new(
            "rust test output",
            r"^---- .+ stdout ----$",
            Detector::until(r"^---- .+ stdout ----$|^failures:$|^test result:", false),
            Error,
        ),
        Detector::new("rust failure list", r"^failures:$", Blank, Error),
        // A rustc diagnostic continues with indented lines, `-->`, gutters
        // (`12 |`, `2 + …`), `= note:` and `help:`/`note:` lines.
        Detector::new(
            "rust diagnostic",
            r"^error(\[[A-Z]\d+\])?: ",
            Detector::while_matching(RUST_CONTINUATION),
            Error,
        ),
        Detector::new(
            "rust diagnostic",
            r"^warning(\[[A-Z]\d+\])?: ",
            Detector::while_matching(RUST_CONTINUATION),
            Warning,
        ),
        {
            let mut p = Detector::new(
                "panic",
                r"^thread '.*'( \(\d+\))? panicked at",
                Blank,
                Error,
            );
            p.max_lines = 40;
            p
        },
        Detector::new("test summary", r"^test result: FAILED", Single, Error),
        // ── Python / pytest ──
        Detector::new(
            "python traceback",
            r"^Traceback \(most recent call last\):",
            FirstUnindented,
            Error,
        ),
        Detector::new(
            "pytest failure",
            r"^_{3,} .+ _{3,}$",
            Detector::until(r"^_{3,} .+ _{3,}$|^={3,}", false),
            Error,
        ),
        Detector::new("pytest summary", r"^(FAILED|ERROR) \S", Single, Error),
        Detector::new(
            "pytest section",
            r"^={3,} (FAILURES|ERRORS) ={3,}$",
            Single,
            Error,
        ),
        // ── Go ──
        Detector::new("go test failure", r"^\s*--- FAIL: ", Indented, Error),
        Detector::new(
            "go panic",
            r"^panic: ",
            Detector::until(r"^(FAIL|ok|PASS)\b|^--- |^=== |^exit status \d+", false),
            Error,
        ),
        {
            let mut g = Detector::new(
                "go build failure",
                r"^# [\w./@-]+( \[[\w./@-]+\])?$",
                Detector::while_matching(r"^[^\s#][^:\s]*\.go:\d+|^\s"),
                Error,
            );
            // Only a heading followed by `file.go:L:C:` lines is a build failure.
            g.min_lines = 2;
            g
        },
        Detector::new(
            "go diagnostic",
            r"^\.?[\w./-]+\.go:\d+(:\d+)?: ",
            Single,
            Error,
        ),
        Detector::new("go package failure", r"^FAIL\s", Single, Error),
        // ── JavaScript / TypeScript ──
        Detector::new(
            "test runner failure",
            r"^\s*FAIL\s+\S",
            Detector::until(
                r"^\s*⎯{3,}|^\s*FAIL\s|^\s*(Test Files|Tests|Test Suites):?\s",
                false,
            ),
            Error,
        ),
        Detector::new("failed test", r"^\s*(✗|×|✕|❌)\s", Indented, Error),
        Detector::new(
            "js error",
            r"^\s*(Uncaught )?(\w+Error|Error)( \[[\w-]+\])?: ",
            Single,
            Error,
        ),
        Detector::new(
            "stack trace",
            r"^\s+at\s+\S",
            Detector::while_matching(r"^\s+(at\s|\.\.\.)"),
            Error,
        ),
        Detector::new("typescript error", r"error TS\d+:", Single, Error),
        Detector::new("lint error", r"^\s*\d+:\d+\s+error\s", Single, Error),
        Detector::new("lint warning", r"^\s*\d+:\d+\s+warning\s", Single, Warning),
        Detector::new(
            "package manager error",
            r"^npm (ERR!|error) |ERR_PNPM_|ELIFECYCLE|YN0001|^error\s",
            Single,
            Error,
        ),
        // ── DevOps ──
        {
            let mut t = Detector::new(
                "terraform diagnostic",
                r"^╷",
                Detector::until(r"^╵", true),
                Error,
            );
            t.warning_if = Some(Regex::new(r"^│ Warning:").expect("regex"));
            t
        },
        Detector::new("kubectl error", r"^Error from server", Single, Error),
        Detector::new(
            "docker error",
            r"^(ERROR|Error response from daemon|failed to solve)\b|exited with code [1-9]",
            Single,
            Error,
        ),
        Detector::new("uv error", r"^\s*× ", Indented, Error),
        // ── Generic ──
        Detector::new(
            "error",
            r"(?i)^\s*(error|fatal|exception|panic)(\[[\w-]+\])?\s*[:!]",
            Single,
            Error,
        ),
        Detector::new(
            "failure",
            r"\bFAILED\b|\bFAILURE\b|^\s*FAIL\b",
            Single,
            Error,
        ),
        Detector::new("warning", r"(?i)^\s*(warn|warning)\s*[:!]", Single, Warning),
    ];
    // The Go stack trace of a panic can be long; the rest stay readable.
    for det in d.iter_mut() {
        if det.kind == "go panic" {
            det.max_lines = 120;
        }
    }
    d
});

/// The built-in detectors, in priority order.
pub fn builtin() -> &'static [Detector] {
    &BUILTIN
}

/// Find the diagnostic blocks of `lines`. `extra` detectors (from rules) are
/// tried before the built-in ones. Blocks never overlap.
pub fn detect(lines: &[String], extra: &[Detector]) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = &lines[i];
        let mut found = None;
        for det in extra.iter().chain(builtin().iter()) {
            if !det.start.is_match(line) {
                continue;
            }
            let end = det.block_end(lines, i);
            if end + 1 - i < det.min_lines {
                continue;
            }
            let mut severity = det.severity;
            if let Some(w) = &det.warning_if {
                if lines[i..=end].iter().any(|l| w.is_match(l)) {
                    severity = Severity::Warning;
                }
            }
            found = Some(Block {
                start: i,
                end,
                severity,
                kind: det.kind.clone(),
            });
            break;
        }
        match found {
            Some(b) => {
                i = b.end + 1;
                blocks.push(b);
            }
            None => i += 1,
        }
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<String> {
        s.lines().map(String::from).collect()
    }

    #[test]
    fn rust_diagnostics_are_whole_blocks() {
        let log = lines(
            "   Compiling x\nerror[E0308]: mismatched types\n  --> src/lib.rs:2:3\n   |\n2  | let x: i32 = \"s\";\n\nwarning: unused\n --> a.rs:1:1\n\nerror: could not compile `x`",
        );
        let b = detect(&log, &[]);
        assert_eq!(b.len(), 3, "{b:?}");
        assert_eq!(
            (b[0].start, b[0].end, b[0].severity),
            (1, 4, Severity::Error)
        );
        assert_eq!(b[1].severity, Severity::Warning);
    }

    #[test]
    fn python_traceback_ends_on_the_exception_line() {
        let log = lines(
            "start\nTraceback (most recent call last):\n  File \"a.py\", line 1, in <module>\n    a()\nKeyError: 'clé'\nafter",
        );
        let b = detect(&log, &[]);
        assert_eq!((b[0].start, b[0].end), (1, 4));
    }

    #[test]
    fn terraform_boxes_and_warning_downgrade() {
        let log = lines("╷\n│ Error: Invalid reference\n│ \n╵\n╷\n│ Warning: Deprecated\n╵");
        let b = detect(&log, &[]);
        assert_eq!(b.len(), 2);
        assert_eq!((b[0].end, b[0].severity), (3, Severity::Error));
        assert_eq!(b[1].severity, Severity::Warning);
    }

    #[test]
    fn success_summaries_are_not_errors() {
        let log = lines(
            "test result: ok. 3 passed; 0 failed; 0 ignored\nok  \texample.com/x\t0.1s\n=== 30 passed in 0.03s ===\nFound 0 errors.",
        );
        assert!(detect(&log, &[]).is_empty(), "{:?}", detect(&log, &[]));
    }

    #[test]
    fn go_build_heading_needs_diagnostics() {
        let log = lines("# example.com/x [example.com/x.test]\nutil/util.go:4:28: undefined: y\nFAIL\tx [build failed]");
        let b = detect(&log, &[]);
        assert_eq!(
            (b[0].start, b[0].end, b[0].kind.as_str()),
            (0, 1, "go build failure")
        );
        let md = lines("# Title\n\nSome text");
        assert!(detect(&md, &[]).is_empty());
    }
}
