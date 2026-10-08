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
use std::path::Path;

/// Hits listed in the transcript; the rest are counted.
const SEARCH_SHOWN: usize = 20;

pub fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

/// One line of a unified diff, coloured.
/// Added lines and their counts: pale green.
pub fn added_style() -> Style {
    Style::default().fg(Color::Rgb(152, 205, 170))
}

/// Removed lines and their counts: pale red.
pub fn removed_style() -> Style {
    Style::default().fg(Color::Rgb(224, 153, 153))
}

/// `+N −N` as two spans, each in its colour (the signs carry the meaning
/// without colours).
pub fn counts(
    added: impl std::fmt::Display,
    removed: impl std::fmt::Display,
) -> Vec<Span<'static>> {
    vec![
        Span::styled(format!("+{added}"), added_style()),
        Span::raw(" "),
        Span::styled(format!("−{removed}"), removed_style()),
    ]
}

pub fn diff_line(l: &str) -> Line<'static> {
    let style = if l.starts_with("+++") || l.starts_with("---") {
        Style::default().add_modifier(Modifier::BOLD)
    } else if l.starts_with('+') {
        added_style()
    } else if l.starts_with('-') {
        removed_style()
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
    // Final duration (from the engine) once done, even at 0 ms; the
    // elapsed time so far, measured here, while it runs.
    let took = match (t.status, t.started, t.duration_ms) {
        (CallStatus::Running, Some(s), _) => Some(format!(
            "  {} so far",
            cersei_types::duration::display_ms(s.elapsed())
        )),
        (CallStatus::Running, None, _) | (_, _, None) => None,
        (_, _, Some(ms)) => Some(format!("  {}", cersei_types::duration::display_ms_u64(ms))),
    };
    if let Some(took) = took {
        spans.push(Span::styled(took, dim()));
    }
    if let Some(p) = &t.progress {
        spans.push(Span::styled(
            format!("  {}", p.lines().last().unwrap_or("")),
            dim(),
        ));
    }
    Line::from(spans)
}

fn agent_lines(a: &crate::state::AgentCell) -> Vec<Line<'static>> {
    use cersei_agent::agents::InstanceState as S;
    let style = match a.state {
        S::Completed => Style::default().fg(Color::Green),
        S::Failed => Style::default().fg(Color::Red),
        S::Incomplete | S::Cancelled | S::Cancelling => Style::default().fg(Color::Yellow),
        _ => Style::default().fg(Color::Cyan),
    };
    let reasoning = if a.info.reasoning.applied == "(none)" {
        String::new()
    } else {
        format!(" · {}", a.info.reasoning.applied)
    };
    let mut head = vec![
        Span::styled("  ⤷ agent ", style),
        Span::styled(
            a.info.profile.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                " · {}{reasoning} · {}",
                a.info.model.applied,
                a.state.as_str()
            ),
            dim(),
        ),
    ];
    if a.result.is_none() {
        head.push(Span::styled(
            format!(
                "  {} so far",
                cersei_types::duration::display_ms(a.started.elapsed())
            ),
            dim(),
        ));
    }
    let mut v = vec![
        Line::from(head),
        Line::from(Span::styled(format!("    {}", a.info.task), dim())),
    ];
    if let Some(r) = &a.reason {
        v.push(Line::from(Span::styled(format!("    {r}"), dim())));
    }
    for t in &a.tools {
        let mut l = tool_line(t);
        l.spans.insert(0, Span::raw("  "));
        v.push(l);
    }
    if let Some(r) = &a.result {
        let mut s = format!(
            "    {} in {} · {} turn(s)",
            r.status,
            cersei_types::duration::display_ms_u64(r.duration_ms),
            r.turns
        );
        if !r.files_changed.is_empty() {
            s.push_str(&format!(" · files: {}", r.files_changed.join(", ")));
        }
        if let Some(e) = &r.error {
            s.push_str(&format!(" · {e}"));
        }
        v.push(Line::from(Span::styled(s, style)));
        for w in &r.warnings {
            v.push(Line::from(Span::styled(format!("    ! {w}"), dim())));
        }
    }
    v
}

/// Lines of a cell, without link destinations.
pub fn cell_lines(cell: &Cell, show_thinking: bool) -> Vec<Line<'static>> {
    cell_rich_lines(cell, show_thinking)
        .into_iter()
        .map(|r| r.line)
        .collect()
}

/// Lines of a cell, with the web destinations of an answer's links.
pub fn cell_rich_lines(cell: &Cell, show_thinking: bool) -> Vec<markdown::RichLine> {
    if let Cell::Assistant { text, .. } = cell {
        let mut v = vec![markdown::RichLine::default()];
        v.extend(markdown::render_rich(text));
        return v;
    }
    plain_cell_lines(cell, show_thinking)
        .into_iter()
        .map(markdown::RichLine::from)
        .collect()
}

fn plain_cell_lines(cell: &Cell, show_thinking: bool) -> Vec<Line<'static>> {
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
                Span::styled(
                    request
                        .agent_id
                        .as_deref()
                        .map(|a| format!("(sub-agent {a}) "))
                        .unwrap_or_default(),
                    dim(),
                ),
                what,
            ])];
            if let Some(p) = &request.preview {
                // A preview: nothing is written yet (not in the totals).
                for f in &p.files {
                    let mut spans = vec![Span::styled(format!("      {} (", f.path), dim())];
                    spans.extend(counts(f.added, f.removed));
                    spans.push(Span::styled(", preview)", dim()));
                    v.push(Line::from(spans));
                }
            }
            v
        }
        Cell::Edits { files } => files
            .iter()
            .map(|f| {
                let mut spans = vec![
                    Span::styled("  ✎ ", Style::default().fg(Color::Green)),
                    Span::raw(format!("{} written ", f.path)),
                ];
                if f.binary {
                    spans.push(Span::styled("(binary)", dim()));
                } else {
                    spans.extend(counts(f.added, f.removed));
                }
                Line::from(spans)
            })
            .collect(),
        Cell::Notice(t) => vec![Line::from(Span::styled(format!("· {t}"), dim()))],
        Cell::Agent(a) => agent_lines(a),
        Cell::Search {
            query,
            status,
            hits,
            omitted,
            notes,
            elapsed_ms,
        } => {
            let mut lines = vec![Line::from(vec![
                Span::styled("⌕ ", Style::default().fg(Color::Cyan)),
                Span::raw(format!("{query}  ")),
                Span::styled(
                    format!("{} hit(s), {status}, {elapsed_ms} ms", hits.len()),
                    dim(),
                ),
            ])];
            for h in hits.iter().take(SEARCH_SHOWN) {
                lines.push(Line::from(vec![
                    Span::styled(format!("  {}:{}:{} ", h.path, h.line, h.column), dim()),
                    Span::raw(h.text.trim().to_string()),
                ]));
            }
            let more = hits.len().saturating_sub(SEARCH_SHOWN) + omitted;
            if more > 0 {
                lines.push(Line::from(Span::styled(
                    format!("  … {more} more (narrow the search)"),
                    dim(),
                )));
            }
            if status != "complete" && hits.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  no hit, but the search was not complete: absence proves nothing",
                    dim(),
                )));
            }
            for n in notes {
                lines.push(Line::from(Span::styled(format!("  · {n}"), dim())));
            }
            lines
        }
        Cell::Error(t) => vec![Line::from(Span::styled(
            format!("✗ {t}"),
            Style::default().fg(Color::Red),
        ))],
        Cell::RunEnd {
            outcome,
            error,
            elapsed,
        } => {
            let took = cersei_types::duration::display_ms(*elapsed);
            let (text, style) = match outcome {
                RunOutcome::Succeeded => (format!("── done in {took}"), dim()),
                RunOutcome::Incomplete => (
                    format!(
                        "── incomplete after {took}: {}",
                        error.clone().unwrap_or_default()
                    ),
                    Style::default().fg(Color::Yellow),
                ),
                RunOutcome::Cancelled => (
                    format!("── cancelled after {took} (effects already made remain)"),
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

/// A project folder in a few columns: the home folder as `~`, then, when
/// too long, its last components behind `…/` (at least the folder and its
/// parent) and a short mark of the full path, so two projects whose
/// shortened names coincide still differ. The full path is in `/session`
/// and the session picker.
pub fn short_path(path: &str, home: Option<&Path>, max: usize) -> String {
    let p = Path::new(path);
    let shown = match home.and_then(|h| p.strip_prefix(h).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.to_string(),
    };
    if shown.chars().count() <= max {
        return shown;
    }
    let parts: Vec<&str> = shown.split('/').filter(|c| !c.is_empty()).collect();
    let mut keep = parts.len().min(2);
    while keep < parts.len() {
        let candidate = format!("…/{}", parts[parts.len() - keep - 1..].join("/"));
        if candidate.chars().count() > max {
            break;
        }
        keep += 1;
    }
    let mut h: u32 = 0x811c9dc5;
    for byte in path.bytes() {
        h ^= u32::from(byte);
        h = h.wrapping_mul(0x01000193);
    }
    format!(
        "…/{} #{:04x}",
        parts[parts.len() - keep..].join("/"),
        h & 0xffff
    )
}

/// The status bar: model, profile, context, cost, activity, project.
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
        Some(None) => spans.push(Span::styled("  Not priced", dim())),
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
    // The project last: what is happening comes first on a narrow terminal.
    if !s.working_dir.is_empty() {
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        spans.push(Span::styled(
            format!("  ⌂ {}", short_path(&s.working_dir, home.as_deref(), 32)),
            dim(),
        ));
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
    /// Activate web links (OSC 8).
    pub hyperlinks: bool,
}

/// Draw the live area: uncommitted cells (tail), then popup, composer and
/// status. Returns nothing; sets the cursor in the composer.
pub fn draw_live(f: &mut Frame, app: &App, composer: &Composer, o: &Overlay) {
    let area = f.area();
    f.render_widget(Clear, area);
    if area.height < 5 || area.width < 20 {
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
                                                          // The totals line, then the status line.
    let status_h = 2usize;
    let popup_h = o
        .popup
        .map(|(items, _, _)| items.len().min(8) + 1)
        .unwrap_or(0);
    let total = area.height as usize;
    let body_h = total.saturating_sub(composer_h + status_h + popup_h);

    // Body: the tail of the live cells (and an approval hint).
    let mut body: Vec<markdown::RichLine> = Vec::new();
    for c in &app.cells[app.committed..] {
        body.extend(markdown::wrap_rich_all(
            &cell_rich_lines(c, o.show_thinking),
            width,
        ));
    }
    if !app.pending.is_empty() {
        let req = &app.pending[0];
        let hint = Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD);
        let mut spans = vec![Span::styled(format!("  {} ", req.tool), hint)];
        if let Some(p) = &req.preview {
            spans.push(Span::styled("(", hint));
            spans.extend(counts(p.added(), p.removed()));
            spans.push(Span::styled(")", hint));
        }
        spans.push(Span::styled(
            format!(
                " — [y] allow  [a] allow for the session  [n] reject  [d] diff/details{}",
                if o.focus_approval {
                    ""
                } else {
                    "  (Tab: focus)"
                }
            ),
            hint,
        ));
        body.push(markdown::RichLine::from(Line::from(spans)));
    }
    if let Some(m) = o.message {
        body.push(markdown::RichLine::from(Line::from(Span::styled(
            m.to_string(),
            Style::default().fg(Color::Yellow),
        ))));
    }
    let skip = body.len().saturating_sub(body_h);
    let body: Vec<markdown::RichLine> = body.into_iter().skip(skip).collect();
    let mut y = area.y;
    let body_area = Rect::new(area.x, y, area.width, body_h as u16);
    let placed = crate::links::draw_rich(f.buffer_mut(), body_area, &body);
    if o.hyperlinks {
        crate::links::apply(f.buffer_mut(), &placed);
    }
    y += body_h as u16;

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
        Paragraph::new(totals_line(app)),
        Rect::new(
            area.x,
            y.min(area.bottom().saturating_sub(2)),
            area.width,
            1,
        ),
    );
    f.render_widget(
        Paragraph::new(status_line(app, o.spinner, o.maintenance)),
        Rect::new(
            area.x,
            (y + 1).min(area.bottom().saturating_sub(1)),
            area.width,
            1,
        ),
    );
}

/// `+N −N`: lines added and removed by the changes applied to the open
/// session's workspace (shown from `+0 −0`, at rest too).
pub fn totals_line(app: &App) -> Line<'static> {
    let mut spans = vec![Span::raw("  ")];
    spans.extend(counts(app.totals.added, app.totals.removed));
    Line::from(spans)
}
