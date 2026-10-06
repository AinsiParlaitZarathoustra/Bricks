//! Inspector windows, built from the engine's public views (snapshot,
//! tools, MCP statuses, sessions) and the presentation state. Never shows
//! API keys or authentication headers: the views do not carry them.

use crate::overlay::{Overlay, PickAction, PickItem};
use crate::state::{App, Maintenance};
use crate::ui::Ui;
use crate::view::diff_line;
use cersei_agent::control::{Controller, SessionSummary, Snapshot};
use ratatui::text::Line;

fn l(s: impl Into<String>) -> Line<'static> {
    Line::from(s.into())
}

fn cost(u: &cersei_types::Usage) -> String {
    match u.cost_usd {
        Some(c) => format!("${c:.4} (estimated from the configured prices)"),
        None => "unknown — no price is configured for this model (not zero)".into(),
    }
}

pub fn context(s: &Snapshot) -> Overlay {
    let c = &s.context;
    let mut v = vec![
        l(format!("model: {}", c.context_window.model)),
        l(format!(
            "used: {} tokens ({:?}: {})",
            c.context_used.tokens, c.context_used.provenance, c.context_used.method
        )),
        l(format!(
            "  measured {} + estimated {}, upper bound {}",
            c.context_used.measured_tokens,
            c.context_used.estimated_tokens,
            c.context_used.upper_bound
        )),
        l(format!(
            "prompt budget: {} tokens (output reserve {}, margin {})",
            c.input_limit, c.reserved_output, c.margin
        )),
        l(format!(
            "window: {}",
            c.context_window
                .total
                .map(|t| format!("{t} tokens"))
                .unwrap_or_else(|| "not stated by the configuration".into())
        )),
        l(format!(
            "occupation: {:.1}% of the prompt budget",
            c.fraction_used() * 100.0
        )),
        l(""),
        l(format!(
            "session consumption (cumulative, never decreases): {} tokens over {} requests",
            c.total_tokens, c.totals.requests
        )),
    ];
    for n in &c.notes {
        v.push(l(format!("note: {n}")));
    }
    Overlay::text("context", v)
}

pub fn costs(s: &Snapshot, app: &App) -> Overlay {
    let u = &s.usage;
    let mut v = vec![
        l(format!(
            "session: input {} · cache read {} · cache write {} · output {} (reasoning {})",
            u.input_tokens,
            u.cache_read_input_tokens,
            u.cache_creation_input_tokens,
            u.output_tokens,
            u.reasoning_tokens
        )),
        l(format!("cost: {}", cost(u))),
        l(""),
        l("per response (observed usage reported by the server):"),
    ];
    for (i, t) in app.status.turns_usage.iter().enumerate() {
        v.push(l(format!(
            "  {:>3}. in {} · out {} · {}",
            i + 1,
            t.input_tokens,
            t.output_tokens,
            t.cost_usd
                .map(|c| format!("${c:.4}"))
                .unwrap_or_else(|| "cost unknown".into())
        )));
    }
    Overlay::text("cost", v)
}

pub fn session(s: &Snapshot) -> Overlay {
    Overlay::text(
        "session",
        vec![
            l(format!("id: {}", s.session_id)),
            l(format!(
                "title: {}",
                if s.title.is_empty() {
                    "(none yet)"
                } else {
                    &s.title
                }
            )),
            l(format!("working directory: {}", s.working_dir.display())),
            l(format!(
                "model: {}{}",
                s.model,
                s.reasoning
                    .as_ref()
                    .map(|r| format!(" ({r})"))
                    .unwrap_or_default()
            )),
            l(format!("activity: {:?}", s.activity)),
            l(format!(
                "long-term memory space: {}",
                s.memory_space
                    .clone()
                    .unwrap_or_else(|| "none (memory disabled)".into())
            )),
            l(""),
            l("Resume later with `bricks resume <id>` or /resume."),
            l("The persistent shell is not stored: a resumed session starts a new one"),
            l("(its processes, background tasks and variables are not restored)."),
        ],
    )
}

pub fn memory(s: &Snapshot, app: &App) -> Overlay {
    let mut v = vec![l(format!(
        "space: {}",
        s.memory_space
            .clone()
            .unwrap_or_else(|| "none — enable [memory] in bricks.toml".into())
    ))];
    match &app.recall {
        Some(r) => v.push(l(format!(
            "last recall: {} item(s), ~{} tokens of {} allowed, {} left out by the budget",
            r.items, r.tokens, r.budget, r.omitted
        ))),
        None => v.push(l("no recall in this session yet")),
    }
    match &app.maintenance {
        None => v.push(l("maintenance: none yet")),
        Some(Maintenance::Running) => v.push(l(
            "maintenance: running (Ctrl+C cancels it; pending work resumes later)",
        )),
        Some(Maintenance::Done {
            outcome,
            report,
            error,
        }) => {
            v.push(l(format!("last maintenance: {outcome:?}")));
            if let Some(r) = report {
                v.push(l(format!(
                    "  extracted {} · facts created {} · updated {} · embedded {} · failed {} · pending {}",
                    r.extracted, r.facts_created, r.facts_updated, r.embedded, r.failed, r.pending
                )));
                for e in &r.errors {
                    v.push(l(format!("  error: {e}")));
                }
            }
            if let Some(e) = error {
                v.push(l(format!("  error: {e}")));
            }
        }
    }
    Overlay::text("memory", v)
}

pub fn config(s: &Snapshot) -> Overlay {
    let rules = serde_json::to_string_pretty(&s.approval_rules).unwrap_or_default();
    let mut v = vec![
        l(format!("working directory: {}", s.working_dir.display())),
        l(format!(
            "model: {}{}",
            s.model,
            s.reasoning
                .as_ref()
                .map(|r| format!(" ({r})"))
                .unwrap_or_default()
        )),
        l(format!("approvals asked interactively: {}", s.interactive)),
        l(format!(
            "allowed for this session: {}",
            if s.allowed_for_session.is_empty() {
                "(none)".to_string()
            } else {
                s.allowed_for_session.join(", ")
            }
        )),
        l(""),
        l("approval policy ([permissions] in bricks.toml):"),
    ];
    v.extend(rules.lines().map(|x| l(format!("  {x}"))));
    v.push(l(""));
    v.push(l("Keys and authentication headers are never shown here."));
    Overlay::text("configuration", v)
}

pub fn tools(ctl: &Controller) -> Overlay {
    let v = ctl
        .tools()
        .into_iter()
        .map(|t| {
            l(format!(
                "{:<18} {:<10} {}",
                t.name,
                t.level,
                t.description.lines().next().unwrap_or("")
            ))
        })
        .collect();
    Overlay::text("tools", v)
}

pub fn mcp(statuses: Vec<(String, String)>) -> Overlay {
    if statuses.is_empty() {
        return Overlay::plain("MCP", "No MCP server is configured for this session.");
    }
    Overlay::text(
        "MCP",
        statuses
            .into_iter()
            .map(|(n, s)| l(format!("{n}: {s}")))
            .collect(),
    )
}

pub fn help() -> Overlay {
    let mut v = vec![
        l("Keys"),
        l("  Enter send · Shift+Enter / Alt+Enter / Ctrl+J newline · `\\` then Enter: newline"),
        l("  @ attach a file or folder · / commands · Tab/Enter accept · Esc close"),
        l("  ↑/↓ prompt history (first/last line) · Ctrl+W delete word · Ctrl+U clear"),
        l("  Ctrl+C cancel the run (idle: clear, twice: quit) · Ctrl+D quit on empty input"),
        l("  Approval: y allow · a allow for the session · n reject · d diff · Tab focus"),
        l("  Ctrl+T live reasoning · Ctrl+O details of the last tools"),
        l(""),
        l("Commands"),
    ];
    for c in crate::commands::COMMANDS {
        let aliases = if c.aliases.is_empty() {
            String::new()
        } else {
            format!(" (also /{})", c.aliases.join(", /"))
        };
        v.push(l(format!(
            "  /{} {}{aliases} — {}",
            c.name, c.args, c.description
        )));
    }
    Overlay::text("help", v)
}

/// Proposed change, changes applied in this session, and the repository's
/// own diff: three different things, shown apart.
pub fn diff(ui: &Ui) -> Overlay {
    let mut v = vec![l("── proposed (waiting for a decision)")];
    if ui.app.pending.is_empty() {
        v.push(l("  (none)"));
    }
    for p in &ui.app.pending {
        match &p.preview {
            Some(prev) => {
                for f in &prev.files {
                    v.push(l(format!(
                        "  {} {} (+{} −{})",
                        p.tool, f.path, f.added, f.removed
                    )));
                    v.extend(f.diff.lines().map(diff_line));
                }
            }
            None => v.push(l(format!(
                "  {} — no file preview (command or MCP call)",
                p.tool
            ))),
        }
    }
    v.push(l(""));
    v.push(l("── applied by the agent in this session"));
    if ui.app.applied.is_empty() {
        v.push(l("  (none)"));
    }
    for (tool, files) in &ui.app.applied {
        for f in files {
            v.push(l(format!(
                "  {tool}: {} ({:?}, +{} −{})",
                f.path, f.kind, f.added, f.removed
            )));
        }
    }
    v.push(l(
        "  Shell commands and MCP tools may have other effects: they are not file diffs.",
    ));
    v.push(l(""));
    v.push(l(
        "── git diff of the working tree (everything, including your own changes)",
    ));
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(&ui.working_dir)
        .args(["diff", "--no-color", "--stat", "--patch"])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let text = String::from_utf8_lossy(&o.stdout);
            if text.trim().is_empty() {
                v.push(l("  (no unstaged change)"));
            }
            let lines: Vec<&str> = text.lines().collect();
            v.extend(lines.iter().take(3000).map(|x| diff_line(x)));
            if lines.len() > 3000 {
                v.push(l(format!("  … {} more lines", lines.len() - 3000)));
            }
        }
        _ => v.push(l("  (not a git repository, or git is unavailable)")),
    }
    Overlay::text("diff", v)
}

pub fn sessions(list: Vec<SessionSummary>, current: &str) -> Overlay {
    let items = list
        .into_iter()
        .map(|s| {
            let when = chrono::DateTime::from_timestamp_millis(s.updated_at)
                .map(|d| {
                    d.with_timezone(&chrono::Local)
                        .format("%Y-%m-%d %H:%M")
                        .to_string()
                })
                .unwrap_or_default();
            PickItem {
                label: format!(
                    "{}{}",
                    if s.id == current { "● " } else { "  " },
                    if s.title.is_empty() {
                        s.id.clone()
                    } else {
                        s.title.clone()
                    }
                ),
                detail: format!(
                    "{when} · {} msg · {} · {}",
                    s.message_count,
                    s.model.unwrap_or_else(|| "?".into()),
                    s.working_dir
                        .map(|w| w.display().to_string())
                        .unwrap_or_default()
                ),
                value: Some(s.id),
            }
        })
        .collect();
    Overlay::Picker {
        title: "sessions".into(),
        items,
        filter: String::new(),
        selected: 0,
        action: PickAction::Session,
    }
}
