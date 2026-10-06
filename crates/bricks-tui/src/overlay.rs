//! Temporary full-screen windows (on the alternate screen, so the
//! transcript in the scrollback is left untouched): text inspectors and
//! pickers.

use crate::view::dim;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

/// What a picker selection does.
#[derive(Debug, Clone, PartialEq)]
pub enum PickAction {
    /// A model was chosen; profiles may follow.
    Model,
    /// A reasoning profile of `model` (`None`: the model's default).
    Profile {
        model: String,
    },
    Session,
}

#[derive(Debug, Clone)]
pub struct PickItem {
    pub label: String,
    pub detail: String,
    pub value: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Overlay {
    Text {
        title: String,
        lines: Vec<Line<'static>>,
        scroll: usize,
    },
    Picker {
        title: String,
        items: Vec<PickItem>,
        filter: String,
        selected: usize,
        action: PickAction,
    },
}

impl Overlay {
    pub fn text(title: impl Into<String>, lines: Vec<Line<'static>>) -> Self {
        Overlay::Text {
            title: title.into(),
            lines,
            scroll: 0,
        }
    }

    pub fn plain(title: impl Into<String>, text: &str) -> Self {
        Self::text(
            title,
            text.lines().map(|l| Line::from(l.to_string())).collect(),
        )
    }

    /// Picker items matching the filter.
    pub fn visible(&self) -> Vec<&PickItem> {
        match self {
            Overlay::Picker { items, filter, .. } => {
                let f = filter.to_lowercase();
                items
                    .iter()
                    .filter(|i| {
                        f.is_empty()
                            || i.label.to_lowercase().contains(&f)
                            || i.detail.to_lowercase().contains(&f)
                    })
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    pub fn scroll_by(&mut self, delta: isize, page: usize) {
        let n = self.visible().len();
        match self {
            Overlay::Text { lines, scroll, .. } => {
                let max = lines.len().saturating_sub(page);
                *scroll = (*scroll as isize + delta).clamp(0, max as isize) as usize;
            }
            Overlay::Picker { selected, .. } => {
                *selected =
                    (*selected as isize + delta).clamp(0, n.saturating_sub(1) as isize) as usize;
            }
        }
    }
}

pub fn draw(f: &mut Frame, o: &Overlay) {
    let area = f.area();
    f.render_widget(Clear, area);
    let inner = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1));
    match o {
        Overlay::Text {
            title,
            lines,
            scroll,
        } => {
            let page = inner.height.saturating_sub(2) as usize;
            let width = inner.width.saturating_sub(2) as usize;
            let wrapped = crate::markdown::wrap_all(lines, width);
            let shown: Vec<Line> = wrapped.into_iter().skip(*scroll).take(page).collect();
            f.render_widget(
                Paragraph::new(shown).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(format!(" {title} ")),
                ),
                inner,
            );
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    " ↑↓ PgUp PgDn scroll · Esc/q close (the transcript is untouched)",
                    dim(),
                ))),
                Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1),
            );
        }
        Overlay::Picker {
            title,
            filter,
            selected,
            ..
        } => {
            let visible = o.visible();
            let page = inner.height.saturating_sub(3) as usize;
            let start = selected.saturating_sub(page.saturating_sub(1));
            let mut lines = vec![Line::from(vec![
                Span::styled(" filter: ", dim()),
                Span::raw(filter.clone()),
            ])];
            if visible.is_empty() {
                lines.push(Line::from(Span::styled("  (nothing matches)", dim())));
            }
            for (i, it) in visible.iter().enumerate().skip(start).take(page) {
                let style = if i == *selected {
                    Style::default().fg(Color::Black).bg(Color::Cyan)
                } else {
                    Style::default()
                };
                lines.push(Line::from(vec![
                    Span::styled(
                        format!(" {} ", it.label),
                        style.add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(" {}", it.detail),
                        if i == *selected { style } else { dim() },
                    ),
                ]));
            }
            f.render_widget(
                Paragraph::new(lines).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(format!(" {title} ")),
                ),
                inner,
            );
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    " type to filter · ↑↓ choose · Enter select · Esc close",
                    dim(),
                ))),
                Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1),
            );
        }
    }
}
