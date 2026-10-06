//! Structured tool results and their uniform rendering.
//!
//! A [`ToolReport`] keeps the data of a result apart from its text: status,
//! the real exit code when a process exited normally (and none otherwise),
//! the cause of an interruption, the captured streams as they were captured
//! (separate, or announced as combined), a diagnostic suggestion and notes.
//! File tools have no exit code and none is invented for them.
//!
//! The agent renders every result the same way:
//!
//! ```text
//! ✓ [Bash] Succès (0.42s) — code 0
//! --- stdout ---
//! …
//! ```
//!
//! The header describes the call; it is not part of the tool's output, and
//! output compression never sees it.

use serde::Serialize;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Success,
    Failure,
    TimedOut,
    Cancelled,
    /// Started and still running (a background task).
    Running,
}

impl ToolStatus {
    pub fn is_error(self) -> bool {
        matches!(
            self,
            ToolStatus::Failure | ToolStatus::TimedOut | ToolStatus::Cancelled
        )
    }
}

/// What the tool produced, as it was captured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolBody {
    /// The output of a tool that is not a process (file text, a listing, a
    /// message).
    Text(String),
    /// A process's stdout and stderr, captured separately. Their relative
    /// order is not known and is not reconstructed.
    Streams { stdout: String, stderr: String },
    /// Both streams in one (a terminal, a transcript): announced as such.
    Combined(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolReport {
    pub status: ToolStatus,
    /// Exit code of a process that exited normally. `None` for a timeout, a
    /// signal, a launch failure, and for tools that are not processes.
    pub exit_code: Option<i32>,
    /// Why it stopped when it did not exit normally (signal, timeout,
    /// launch failure).
    pub termination: Option<String>,
    /// Measured by the tool (monotonic clock); the agent uses its own
    /// measurement of the call when absent.
    pub duration: Option<Duration>,
    /// For a timeout: the limit that expired.
    pub timeout: Option<Duration>,
    pub body: ToolBody,
    /// A short hint tied to an identified error.
    pub suggestion: Option<String>,
    /// Facts about the run: session reset, partial output, raw locations.
    pub notes: Vec<String>,
    /// Structured data for programs (task id, cwd, …).
    pub data: Option<serde_json::Value>,
}

impl ToolReport {
    pub fn new(status: ToolStatus, body: ToolBody) -> Self {
        Self {
            status,
            exit_code: None,
            termination: None,
            duration: None,
            timeout: None,
            body,
            suggestion: None,
            notes: Vec::new(),
            data: None,
        }
    }

    /// The text of the output itself (sections for streams), without the
    /// header, notes or suggestion. This is what output compression sees.
    pub fn render_output(&self) -> String {
        match &self.body {
            ToolBody::Text(t) => t.clone(),
            ToolBody::Combined(t) => {
                if t.is_empty() {
                    String::new()
                } else {
                    format!(
                        "--- sortie (stdout et stderr combinés) ---\n{}",
                        ensure_nl(t)
                    )
                }
            }
            ToolBody::Streams { stdout, stderr } => {
                let mut out = String::new();
                if !stdout.is_empty() {
                    out.push_str("--- stdout ---\n");
                    out.push_str(&ensure_nl(stdout));
                }
                if !stderr.is_empty() {
                    out.push_str("--- stderr ---\n");
                    out.push_str(&ensure_nl(stderr));
                }
                out
            }
        }
    }

    /// Header line for `tool`, with `fallback` used when the tool did not
    /// measure its own duration.
    pub fn header(&self, tool: &str, fallback: Duration) -> String {
        let secs = self.duration.unwrap_or(fallback).as_secs_f64();
        let (mark, label) = match self.status {
            ToolStatus::Success => ("✓", "Succès".to_string()),
            ToolStatus::Failure => ("✗", "Échec".to_string()),
            ToolStatus::TimedOut => (
                "⏱",
                match self.timeout {
                    Some(t) => format!("Interrompu après timeout de {}", fmt_secs(t)),
                    None => "Interrompu (timeout)".to_string(),
                },
            ),
            ToolStatus::Cancelled => ("⊘", "Annulé".to_string()),
            ToolStatus::Running => ("…", "En cours".to_string()),
        };
        let mut h = format!("{mark} [{tool}] {label} ({secs:.2}s)");
        match (self.exit_code, &self.termination) {
            (Some(c), _) => h.push_str(&format!(" — code {c}")),
            (None, Some(t)) => h.push_str(&format!(" — {t}")),
            (None, None) => {}
        }
        h
    }

    /// The complete text: header, output (`output` replaces the raw output
    /// text, e.g. after compression), notes and suggestion.
    pub fn render(&self, tool: &str, fallback: Duration, output: Option<&str>) -> String {
        let mut out = self.header(tool, fallback);
        out.push('\n');
        if self.status == ToolStatus::TimedOut {
            out.push_str(&format!(
                "[Interrompu après timeout de {} — sortie partielle ci-dessous]\n",
                self.timeout.map(fmt_secs).unwrap_or_else(|| "?".into())
            ));
        }
        let body = match output {
            Some(o) => o.to_string(),
            None => self.render_output(),
        };
        if body.is_empty() {
            match (&self.body, self.status) {
                (ToolBody::Streams { .. } | ToolBody::Combined(_), ToolStatus::Success) => {
                    out.push_str("(Commande exécutée avec succès sans sortie)\n")
                }
                (ToolBody::Streams { .. } | ToolBody::Combined(_), _) => {
                    out.push_str("(aucune sortie)\n")
                }
                _ => {}
            }
        } else {
            out.push_str(&ensure_nl(&body));
        }
        if !self.notes.is_empty() {
            out.push_str("--- remarques ---\n");
            for n in &self.notes {
                out.push_str(&format!("- {n}\n"));
            }
        }
        if let Some(s) = &self.suggestion {
            out.push_str("--- suggestion ---\n");
            out.push_str(&ensure_nl(s));
        }
        out.trim_end_matches('\n').to_string()
    }
}

fn ensure_nl(s: &str) -> String {
    if s.ends_with('\n') {
        s.to_string()
    } else {
        format!("{s}\n")
    }
}

fn fmt_secs(d: Duration) -> String {
    let s = d.as_secs_f64();
    if (s - s.round()).abs() < 1e-9 {
        format!("{}s", s.round() as u64)
    } else {
        format!("{s:.1}s")
    }
}

/// A suggestion for a well-identified process failure, or none.
pub fn suggest(status: ToolStatus, exit_code: Option<i32>, stderr: &str) -> Option<String> {
    match (status, exit_code) {
        (ToolStatus::Failure, Some(127)) if stderr.contains("not found") => {
            Some("Vérifiez que l'outil est installé ou disponible dans le PATH.".into())
        }
        (ToolStatus::Failure, Some(126)) => Some(
            "Le fichier n'est pas exécutable ou n'est pas un programme : vérifiez ses droits (chmod +x) \
             ou lancez-le avec son interpréteur."
                .into(),
        ),
        (ToolStatus::TimedOut, _) => Some(
            "Augmentez `timeout` si la commande doit simplement durer plus longtemps, ou lancez-la \
             avec `background: true`."
                .into(),
        ),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn streams(o: &str, e: &str) -> ToolBody {
        ToolBody::Streams {
            stdout: o.into(),
            stderr: e.into(),
        }
    }

    #[test]
    fn success_and_failure_render_like_the_contract() {
        let mut ok = ToolReport::new(ToolStatus::Success, streams("hi", ""));
        ok.exit_code = Some(0);
        ok.duration = Some(Duration::from_millis(420));
        assert_eq!(
            ok.render("Bash", Duration::ZERO, None),
            "✓ [Bash] Succès (0.42s) — code 0\n--- stdout ---\nhi"
        );

        let mut ko = ToolReport::new(
            ToolStatus::Failure,
            streams("", "command not found: foobar\n"),
        );
        ko.exit_code = Some(127);
        ko.duration = Some(Duration::from_millis(1150));
        ko.suggestion = suggest(ko.status, ko.exit_code, "command not found: foobar");
        assert_eq!(
            ko.render("Bash", Duration::ZERO, None),
            "✗ [Bash] Échec (1.15s) — code 127\n--- stderr ---\ncommand not found: foobar\n\
             --- suggestion ---\nVérifiez que l'outil est installé ou disponible dans le PATH."
        );
    }

    #[test]
    fn empty_success_timeouts_and_file_tools() {
        let mut ok = ToolReport::new(ToolStatus::Success, streams("", ""));
        ok.exit_code = Some(0);
        assert!(ok
            .render("Bash", Duration::from_millis(10), None)
            .ends_with("(Commande exécutée avec succès sans sortie)"));

        let mut t = ToolReport::new(ToolStatus::TimedOut, streams("partial", ""));
        t.timeout = Some(Duration::from_secs(120));
        t.termination = Some("aucun code de sortie (processus arrêtés)".into());
        let r = t.render("Bash", Duration::from_secs(121), None);
        assert!(
            r.starts_with("⏱ [Bash] Interrompu après timeout de 120s (121.00s) — aucun code"),
            "{r}"
        );
        assert!(r.contains("[Interrompu après timeout de 120s — sortie partielle ci-dessous]\n--- stdout ---\npartial"));
        assert!(!r.contains("code 0"));

        // A file tool: no exit code is invented.
        let f = ToolReport::new(ToolStatus::Success, ToolBody::Text("  1 | x".into()));
        assert_eq!(
            f.render("Read", Duration::from_millis(3), None),
            "✓ [Read] Succès (0.00s)\n  1 | x"
        );
        assert!(suggest(ToolStatus::Failure, Some(1), "x").is_none());
    }

    #[test]
    fn combined_output_is_announced_and_running_tasks_are_not_success() {
        let c = ToolReport::new(ToolStatus::Success, ToolBody::Combined("both".into()));
        assert!(c
            .render_output()
            .starts_with("--- sortie (stdout et stderr combinés) ---"));
        let r = ToolReport::new(
            ToolStatus::Running,
            ToolBody::Text("task bg-1 started".into()),
        );
        assert!(r
            .render("Bash", Duration::from_millis(50), None)
            .starts_with("… [Bash] En cours"));
        assert!(!ToolStatus::Running.is_error());
    }
}
