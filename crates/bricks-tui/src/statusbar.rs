//! The status bar, composed for the width it really has.
//!
//! Segments — model and effective profile, context, cost, activity,
//! project — each have variants from rich to short. The bar takes the
//! richest arrangement whose width, measured in terminal cells
//! (`unicode-width`, after removing control characters), fits; text is cut
//! at grapheme boundaries. While a run is active its activity (spinner,
//! `working`, `Ctrl+C`) is kept as long as anything is shown.
//!
//! Figures come from the engine's views through `crate::diag`: the cost
//! state (a partial estimate is never shown as a total) and the context
//! provenance (`≈` for estimates).

use crate::diag::{compact, ContextFigures, CostState};
use crate::state::App;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

const SEP: &str = " │ ";

/// Replace characters a terminal would interpret (C0/C1 controls, escape)
/// by a visible placeholder.
pub fn clean(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { '�' } else { c })
        .collect()
}

pub fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// `s` in at most `max` cells, cut at a grapheme boundary with `…`.
pub fn fit(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for g in s.graphemes(true) {
        let w = width(g);
        if used + w + 1 > max {
            break;
        }
        out.push_str(g);
        used += w;
    }
    out.push('…');
    out
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Model,
    Plain,
    Activity,
}

/// The values the bar shows, cleaned.
struct Parts {
    model: String,
    profile: Option<String>,
    context: Option<(String, String, String)>, // detailed, short, minimal
    cost: Option<String>,
    path_full: Option<String>,
    path_short: Option<String>,
    /// (full, medium, short, minimal)
    activity: Option<[String; 4]>,
}

fn parts(app: &App, spinner: &str, maintenance: bool) -> Parts {
    let s = &app.status;
    let choice = app.models.iter().find(|m| m.selection == s.model);
    let model = clean(
        choice
            .map(|m| m.name.as_str())
            .filter(|n| !n.is_empty())
            .unwrap_or(s.model.as_str()),
    );
    let profile = s.effective_reasoning.as_ref().map(|id| {
        clean(
            choice
                .and_then(|m| m.profiles.iter().find(|p| &p.id == id))
                .map(|p| p.label.as_str())
                .unwrap_or(id.as_str()),
        )
    });
    let context = s.context.as_ref().map(|c| {
        let f = ContextFigures::of(c);
        (
            format!(
                "Context {}{} / {} ({})",
                f.mark(),
                compact(f.used),
                compact(c.input_limit),
                f.percent_text()
            ),
            format!("Ctx {}", f.percent_text()),
            f.percent_text(),
        )
    });
    let cost = s
        .total
        .as_ref()
        .map(CostState::of)
        .filter(|c| *c != CostState::NoUsage)
        .map(|c| c.short());
    let (path_full, path_short) = if s.working_dir.is_empty() {
        (None, None)
    } else {
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        let full = clean(&crate::view::short_path(
            &s.working_dir,
            home.as_deref(),
            40,
        ));
        let last = std::path::Path::new(&s.working_dir)
            .file_name()
            .map(|n| format!("…/{}", clean(&n.to_string_lossy())))
            .unwrap_or_else(|| full.clone());
        (Some(full), Some(last))
    };
    let activity = if app.running() {
        Some([
            format!("{spinner} working — Ctrl+C to cancel"),
            format!("{spinner} working · Ctrl+C"),
            format!("{spinner} Ctrl+C"),
            spinner.to_string(),
        ])
    } else if maintenance {
        Some([
            format!("{spinner} updating memory"),
            format!("{spinner} memory"),
            format!("{spinner} memory"),
            spinner.to_string(),
        ])
    } else {
        None
    };
    Parts {
        model,
        profile,
        context,
        cost,
        path_full,
        path_short,
        activity,
    }
}

type Segs = Vec<(String, Kind)>;

fn total(segs: &Segs) -> usize {
    segs.iter().map(|(t, _)| width(t)).sum::<usize>() + width(SEP) * segs.len().saturating_sub(1)
}

/// The arrangements, from the richest.
fn layouts(p: &Parts) -> Vec<Segs> {
    let model_full = match &p.profile {
        Some(r) => format!("{} · {r}", p.model),
        None => p.model.clone(),
    };
    let act = |i: usize| p.activity.as_ref().map(|a| (a[i].clone(), Kind::Activity));
    let ctx = |i: usize| {
        p.context.as_ref().map(|c| {
            (
                match i {
                    0 => c.0.clone(),
                    1 => c.1.clone(),
                    _ => c.2.clone(),
                },
                Kind::Plain,
            )
        })
    };
    let cost = || p.cost.clone().map(|c| (c, Kind::Plain));
    let full_path = || p.path_full.clone().map(|c| (c, Kind::Plain));
    let short_path = || p.path_short.clone().map(|c| (c, Kind::Plain));
    let mk = |items: Vec<Option<(String, Kind)>>| -> Segs { items.into_iter().flatten().collect() };
    vec![
        // Large: everything.
        mk(vec![
            Some((model_full.clone(), Kind::Model)),
            ctx(0),
            cost(),
            act(0),
            full_path(),
        ]),
        // Medium: percentage, project abbreviated.
        mk(vec![
            Some((model_full.clone(), Kind::Model)),
            ctx(1),
            cost(),
            act(0),
            short_path(),
        ]),
        mk(vec![
            Some((model_full.clone(), Kind::Model)),
            ctx(1),
            cost(),
            act(1),
            short_path(),
        ]),
        // Small: model without its profile, no cost before the project.
        mk(vec![
            Some((p.model.clone(), Kind::Model)),
            ctx(1),
            cost(),
            act(1),
        ]),
        mk(vec![
            Some((model_full.clone(), Kind::Model)),
            ctx(2),
            act(2),
        ]),
        mk(vec![Some((p.model.clone(), Kind::Model)), ctx(2), act(2)]),
        // Very small: the model (cut) and the activity.
        mk(vec![Some((p.model.clone(), Kind::Model)), act(3)]),
    ]
}

/// The status bar for `width` cells.
pub fn status_line(
    app: &App,
    spinner: &str,
    maintenance: bool,
    width_cells: usize,
) -> Line<'static> {
    let p = parts(app, spinner, maintenance);
    let all = layouts(&p);
    let mut chosen = all
        .iter()
        .find(|segs| total(segs) <= width_cells)
        .cloned()
        .unwrap_or_else(|| all.last().cloned().unwrap_or_default());
    // Still too wide: shorten the model, keep the activity whole if it fits.
    if total(&chosen) > width_cells {
        let others: usize = chosen
            .iter()
            .filter(|(_, k)| *k != Kind::Model)
            .map(|(t, _)| width(t) + width(SEP))
            .sum();
        let room = width_cells.saturating_sub(others);
        for seg in chosen.iter_mut().filter(|(_, k)| *k == Kind::Model) {
            seg.0 = fit(&seg.0, room);
        }
        chosen.retain(|(t, _)| !t.is_empty());
        if total(&chosen) > width_cells {
            // Only the activity (or the model) remains, cut to the width.
            let keep = chosen
                .iter()
                .find(|(_, k)| *k == Kind::Activity)
                .or_else(|| chosen.first())
                .cloned();
            chosen = keep
                .map(|(t, k)| vec![(fit(&t, width_cells), k)])
                .unwrap_or_default();
        }
    }
    let mut spans = Vec::new();
    for (i, (text, kind)) in chosen.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(
                SEP,
                Style::default().add_modifier(Modifier::DIM),
            ));
        }
        let style = match kind {
            Kind::Model => Style::default().add_modifier(Modifier::BOLD),
            Kind::Activity => Style::default().fg(Color::Yellow),
            Kind::Plain => Style::default().add_modifier(Modifier::DIM),
        };
        spans.push(Span::styled(text, style));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_cuts_at_graphemes_and_counts_cells() {
        assert_eq!(fit("abc", 5), "abc");
        assert_eq!(fit("abcdef", 4), "abc…");
        // CJK: two cells each.
        let cut = fit("漢字漢字漢字", 5);
        assert!(width(&cut) <= 5, "{cut}");
        // Emoji with a modifier stays whole or goes away.
        let cut = fit("👍🏽👍🏽👍🏽", 4);
        assert!(width(&cut) <= 4, "{cut}");
        assert_eq!(
            cut.matches('\u{1F3FD}').count(),
            cut.matches('👍').count(),
            "{cut}"
        );
        assert_eq!(
            fit("é\u{301}é\u{301}é\u{301}", 3),
            "é\u{301}é\u{301}é\u{301}"
        );
        assert_eq!(fit("é\u{301}é\u{301}é\u{301}", 2), "é\u{301}…");
        assert_eq!(fit("x", 0), "");
    }

    #[test]
    fn controls_are_neutralised() {
        assert_eq!(clean("a\u{1b}[31mb\u{7}"), "a�[31mb�");
    }
}
