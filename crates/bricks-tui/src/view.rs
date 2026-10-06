//! Drawing: cells to lines (for the scrollback and the live area), and the
//! live area itself (in-progress cells, approval, popups, composer, status).
//! Drawing only reads state: no engine call, no tool, no file access.

use crate::composer::Composer;
use crate::markdown;
use crate::state::{App, CallStatus, Cell, ToolCall};
use cersei_agent::control::{DecidedBy, Decision, RunOutcome};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;

pub fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

/// One line of a unified diff, coloured.
pub fn diff_line(l: &str) -> Line<'static> {
    let style = if l.starts_with("+++") || l.starts_with("---") {
        Style::default().add_modifier(Modifier::BOLD)
    } else if l.starts_with('+') {
        Style::default().fg(Color::Green)
    } else if l.starts_with('-') {
        Style::default().fg(Color::Red)
    } else if l.starts_with("@@") {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    };
    Line::from(Span::styled(l.to_string(), style))
}

fn tool_line(t: &ToolCall) -> Line<'static> {
    let (mark, style) = match t.status {
        CallStatus::Running => ("◌", Style::default().fg(Color::Yellow)),
        CallStatus::Ok => ("✓", Style::default().fg(Color::Green)),
        CallStatus::Failed => ("✗", Style::default().fg(Color::Red)),
    };
    let mut spans = vec![
        Span::styled(format!("  {mark} "), style),
        Span::styled(
            t.name.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(" {}", t.summary)),
    ];
    if t.status != CallStatus::Running && t.duration_ms > 0 {
        spans.push(Span::styled(
            format!("  {:.1}s", t.duration_ms as f64 / 1000.0),
            dim(),
        ));
    }
    if let Some(p) = &t.progress {
        spans.push(Span::styled(
            format!("  {}", p.lines().last().unwrap_or("")),
            dim(),
        ));
    }
    Line::from(spans)
}

/// Lines of a cell. `live`: the cell is in the live area (show progress,
/// the tail of long texts); otherwise it goes to the scrollback.
pub fn cell_lines(cell: &Cell, show_thinking: bool) -> Vec<Line<'static>> {
    match cell {
        Cell::User { text, attachments } => {
            let mut v = vec![Line::default()];
            for (i, l) in text.lines().enumerate() {
                let p = if i == 0 { "› " } else { "  " };
                v.push(Line::from(vec![
                    Span::styled(
                        p,
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(l.to_string(), Style::default().add_modifier(Modifier::BOLD)),
                ]));
            }
            for a in attachments {
                v.push(Line::from(Span::styled(format!("  📎 {a}"), dim())));
            }
            v
        }
        Cell::Assistant { text, .. } => {
            let mut v = vec![Line::default()];
            v.extend(markdown::render(text));
            v
        }
        Cell::Thinking { text, .. } => {
            let n = text.lines().count().max(1);
            let mut v = vec![Line::from(Span::styled(
                format!(
                    "∴ reasoning ({n} line{}){}",
                    if n > 1 { "s" } else { "" },
                    if show_thinking {
                        ""
                    } else {
                        " — Ctrl+T to show"
                    }
                ),
                dim().add_modifier(Modifier::ITALIC),
            ))];
            if show_thinking {
                v.extend(text.lines().map(|l| {
                    Line::from(Span::styled(
                        format!("  {l}"),
                        dim().add_modifier(Modifier::ITALIC),
                    ))
                }));
            }
            v
        }
        Cell::Tools { calls } => {
            let mut v = Vec::new();
            // Repetitive activity is grouped: more than 6 calls show the
            // first ones, then a count by tool; details stay in the
            // inspector (Ctrl+O).
            if calls.len() > 6 {
                for t in &calls[..3] {
                    v.push(tool_line(t));
                }
                let mut counts: Vec<(String, usize)> = Vec::new();
                for t in &calls[3..] {
                    match counts.iter_mut().find(|(n, _)| *n == t.name) {
                        Some((_, c)) => *c += 1,
                        None => counts.push((t.name.clone(), 1)),
                    }
                }
                let failed = calls
                    .iter()
                    .filter(|t| t.status == CallStatus::Failed)
                    .count();
                let running = calls
                    .iter()
                    .filter(|t| t.status == CallStatus::Running)
                    .count();
                let summary: Vec<String> =
                    counts.iter().map(|(n, c)| format!("{n} ×{c}")).collect();
                v.push(Line::from(Span::styled(
                    format!(
                        "  … {} more ({}){}{} — Ctrl+O for details",
                        calls.len() - 3,
                        summary.join(", "),
                        if failed > 0 {
                            format!(", {failed} failed")
                        } else {
                            String::new()
                        },
                        if running > 0 {
                            format!(", {running} running")
                        } else {
                            String::new()
                        }
                    ),
                    dim(),
                )));
            } else {
                for t in calls {
                    v.push(tool_line(t));
                    if t.status == CallStatus::Failed && !t.output.is_empty() {
                        let first = t
                            .output
                            .lines()
                            .find(|l| !l.trim().is_empty())
                            .unwrap_or("");
                        let first: String = first.chars().take(200).collect();
                        v.push(Line::from(Span::styled(
                            format!("      {first}"),
                            Style::default().fg(Color::Red),
                        )));
                    }
                }
            }
            v
        }
        Cell::Approval {
            request,
            resolution,
        } => {
            let what = match resolution {
                None => Span::styled(
                    "waiting for your decision",
                    Style::default().fg(Color::Yellow),
                ),
                Some((Decision::Allow, _)) => {
                    Span::styled("allowed", Style::default().fg(Color::Green))
                }
                Some((Decision::AllowForSession, _)) => Span::styled(
                    "allowed for this session",
                    Style::default().fg(Color::Green),
                ),
                Some((Decision::Deny, DecidedBy::NonInteractive)) => Span::styled(
                    "needed, nobody could approve",
                    Style::default().fg(Color::Red),
                ),
                Some((Decision::Deny, DecidedBy::Cancelled)) => {
                    Span::styled("cancelled", Style::default().fg(Color::Red))
                }
                Some((Decision::Deny, _)) => {
                    Span::styled("rejected", Style::default().fg(Color::Red))
                }
            };
            let mut v = vec![Line::from(vec![
                Span::styled("  ? ", Style::default().fg(Color::Yellow)),
                Span::styled(
                    format!("{} ", request.tool),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                what,
            ])];
            if let Some(p) = &request.preview {
                for f in &p.files {
                    v.push(Line::from(Span::styled(
                        format!("      {} (+{} −{})", f.path, f.added, f.removed),
                        dim(),
                    )));
                }
            }
            v
        }
        Cell::Edits { files } => files
            .iter()
            .map(|f| {
                Line::from(vec![
                    Span::styled("  ✎ ", Style::default().fg(Color::Green)),
                    Span::raw(format!("{} written ", f.path)),
                    Span::styled(format!("+{} −{}", f.added, f.removed), dim()),
                ])
            })
            .collect(),
        Cell::Notice(t) => vec![Line::from(Span::styled(format!("· {t}"), dim()))],
        Cell::Error(t) => vec![Line::from(Span::styled(
            format!("✗ {t}"),
            Style::default().fg(Color::Red),
        ))],
        Cell::RunEnd {
            outcome,
            error,
            seconds,
        } => {
            let (text, style) = match outcome {
                RunOutcome::Succeeded => (format!("── done in {seconds:.1}s"), dim()),
                RunOutcome::Incomplete => (
                    format!(
                        "── incomplete after {seconds:.1}s: {}",
                        error.clone().unwrap_or_default()
                    ),
                    Style::default().fg(Color::Yellow),
                ),
                RunOutcome::Cancelled => (
                    format!("── cancelled after {seconds:.1}s (effects already made remain)"),
                    Style::default().fg(Color::Yellow),
                ),
                RunOutcome::Failed => (
                    format!("── failed: {}", error.clone().unwrap_or_default()),
                    Style::default().fg(Color::Red),
                ),
            };
            vec![Line::from(Span::styled(text, style))]
        }
    }
}

/// The status bar: model, profile, context, cost, activity.
pub fn status_line(app: &App, spinner: &str, maintenance: bool) -> Line<'static> {
    let s = &app.status;
    let mut spans = vec![Span::styled(
        s.model.clone(),
        Style::default().add_modifier(Modifier::BOLD),
    )];
    if let Some(r) = &s.reasoning {
        spans.push(Span::styled(format!(" · {r}"), dim()));
    }
    if let Some(c) = &s.context {
        let k = |n: u64| {
            if n >= 1000 {
                format!("{:.1}k", n as f64 / 1000.0)
            } else {
                n.to_string()
            }
        };
        let prov = match c.context_used.provenance {
            cersei_agent::Provenance::Measured => "",
            cersei_agent::Provenance::Counted => " counted",
            cersei_agent::Provenance::Mixed => " ~",
            cersei_agent::Provenance::Estimated => " est.",
        };
        spans.push(Span::styled(
            format!(
                "  ctx {}/{}{prov}",
                k(c.context_used.tokens),
                k(c.input_limit)
            ),
            dim(),
        ));
    }
    match s.total.as_ref().map(|u| u.cost_usd) {
        Some(Some(c)) => spans.push(Span::styled(format!("  ${c:.4}"), dim())),
        Some(None) => spans.push(Span::styled("  cost unknown (no price)", dim())),
        None => {}
    }
    if app.running() {
        spans.push(Span::styled(
            format!("  {spinner} working — Ctrl+C to cancel"),
            Style::default().fg(Color::Yellow),
        ));
    } else if maintenance {
        spans.push(Span::styled(format!("  {spinner} updating memory"), dim()));
    }
    Line::from(spans)
}

/// What the live area shows besides the cells.
pub struct Overlay<'a> {
    /// Completion list (mentions or commands): items, selected index, title.
    pub popup: Option<(&'a [String], usize, &'a str)>,
    pub spinner: &'a str,
    pub maintenance: bool,
    pub show_thinking: bool,
    pub focus_approval: bool,
    pub message: Option<&'a str>,
}

/// Draw the live area: uncommitted cells (tail), then popup, composer and
/// status. Returns nothing; sets the cursor in the composer.
pub fn draw_live(f: &mut Frame, app: &App, composer: &Composer, o: &Overlay) {
    let area = f.area();
    f.render_widget(Clear, area);
    if area.height < 4 || area.width < 20 {
        f.render_widget(Paragraph::new("terminal too small"), area);
        return;
    }
    let width = area.width as usize;
    // Composer: chips line + text rows (at most 6 rows shown).
    let (rows, (cr, cc)) = composer.layout(width.saturating_sub(2));
    let shown_rows = rows.len().clamp(1, 6);
    let first_row = cr
        .saturating_sub(shown_rows - 1)
        .min(rows.len().saturating_sub(shown_rows));
    let chips = !composer.attachments().is_empty();
    let composer_h = shown_rows + usize::from(chips) + 1; // + separator
    let status_h = 1usize;
    let popup_h = o
        .popup
        .map(|(items, _, _)| items.len().min(8) + 1)
        .unwrap_or(0);
    let total = area.height as usize;
    let body_h = total.saturating_sub(composer_h + status_h + popup_h);

    // Body: the tail of the live cells (and an approval hint).
    let mut body: Vec<Line<'static>> = Vec::new();
    for c in &app.cells[app.committed..] {
        body.extend(markdown::wrap_all(&cell_lines(c, o.show_thinking), width));
    }
    if !app.pending.is_empty() {
        let req = &app.pending[0];
        body.push(Line::from(Span::styled(
            format!(
                "  {} {} — [y] allow  [a] allow for the session  [n] reject  [d] diff/details{}",
                req.tool,
                req.preview
                    .as_ref()
                    .map(|p| format!("(+{} −{})", p.added(), p.removed()))
                    .unwrap_or_default(),
                if o.focus_approval {
                    ""
                } else {
                    "  (Tab: focus)"
                }
            ),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));
    }
    if let Some(m) = o.message {
        body.push(Line::from(Span::styled(
            m.to_string(),
            Style::default().fg(Color::Yellow),
        )));
    }
    let skip = body.len().saturating_sub(body_h);
    let body: Vec<Line> = body.into_iter().skip(skip).collect();
    let mut y = area.y;
    let body_len = body.len() as u16;
    f.render_widget(
        Paragraph::new(body),
        Rect::new(area.x, y, area.width, body_h as u16),
    );
    y += body_h as u16;
    let _ = body_len;

    if let Some((items, sel, title)) = o.popup {
        let mut lines = vec![Line::from(Span::styled(title.to_string(), dim()))];
        let start = sel.saturating_sub(7);
        for (i, it) in items.iter().enumerate().skip(start).take(8) {
            let style = if i == sel {
                Style::default().fg(Color::Black).bg(Color::Cyan)
            } else {
                Style::default()
            };
            lines.push(Line::from(Span::styled(format!(" {it}"), style)));
        }
        f.render_widget(
            Paragraph::new(lines),
            Rect::new(area.x, y, area.width, popup_h as u16),
        );
        y += popup_h as u16;
    }

    f.render_widget(
        Paragraph::new(Line::from(Span::styled("─".repeat(width), dim()))),
        Rect::new(area.x, y, area.width, 1),
    );
    y += 1;
    if chips {
        let labels: Vec<Span> = composer
            .attachments()
            .iter()
            .map(|a| {
                Span::styled(
                    format!("[{}] ", a.label()),
                    Style::default().fg(Color::Magenta),
                )
            })
            .collect();
        f.render_widget(
            Paragraph::new(Line::from(labels)),
            Rect::new(area.x, y, area.width, 1),
        );
        y += 1;
    }
    let mut text_lines = Vec::new();
    for (i, r) in rows.iter().enumerate().skip(first_row).take(shown_rows) {
        let prefix = if i == 0 { "› " } else { "  " };
        text_lines.push(Line::from(vec![
            Span::styled(prefix, Style::default().fg(Color::Cyan)),
            Span::raw(r.clone()),
        ]));
    }
    if composer.text().is_empty() && !chips {
        text_lines = vec![Line::from(vec![
            Span::styled("› ", Style::default().fg(Color::Cyan)),
            Span::styled(
                "Ask Bricks…  (Enter send · Shift/Alt+Enter newline · @ file · / command)",
                dim(),
            ),
        ])];
    }
    f.render_widget(
        Paragraph::new(text_lines),
        Rect::new(area.x, y, area.width, shown_rows as u16),
    );
    if !o.focus_approval {
        f.set_cursor_position((
            (area.x + 2 + cc as u16).min(area.right().saturating_sub(1)),
            y + (cr - first_row) as u16,
        ));
    }
    y += shown_rows as u16;
    f.render_widget(
        Paragraph::new(status_line(app, o.spinner, o.maintenance)),
        Rect::new(
            area.x,
            y.min(area.bottom().saturating_sub(1)),
            area.width,
            1,
        ),
    );
}
