//! The interface's state machine: gestures in, effects out. No terminal
//! and no engine here — the runtime (`lib.rs`) executes the effects, so
//! this can be tested with plain key events.
//!
//! Keys (documented in docs/cli.md):
//! * Enter sends; Shift+Enter, Alt+Enter or Ctrl+J insert a newline; a
//!   line ending with `\` followed by Enter also becomes a newline
//!   (for terminals that report Shift+Enter as Enter).
//! * A paste (bracketed paste) is inserted as text, never sent.
//! * `@` opens the file list, `/` the command list; Tab or Enter accept,
//!   Esc closes, Ctrl+R refreshes the file index.
//! * ↑/↓ browse the prompt history on the first/last line.
//! * Ctrl+C cancels the run (or the memory maintenance); when idle it
//!   clears the input, and twice in a row quits. Ctrl+D on an empty input
//!   quits.
//! * With a pending approval: y allow, a allow for the session, n reject,
//!   d open the diff; Tab switches between the approval and the input.
//! * Ctrl+T shows/hides the live reasoning, Ctrl+O opens the details of
//!   the last tool calls and reasoning.

use crate::commands::{self, Action, AttachKind, Present};
use crate::composer::{Attachment, Composer};
use crate::mentions::{Entry, FileIndex};
use crate::overlay::{Overlay, PickAction, PickItem};
use crate::state::{App, Cell};
use cersei_agent::control::{ApprovalRequest, Command, Decision, Envelope, Event};
use cersei_tools::preview::ChangePreview;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::text::Line;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// What the runtime must do.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Send(Command),
    /// Needs data from the engine or the system (snapshot, sessions, git).
    Present(Present),
    /// A model was picked: offer its reasoning profiles (or set it).
    ChooseProfile(String),
    Quit,
}

pub enum Popup {
    Mentions(Vec<Entry>),
    Commands(Vec<&'static commands::SlashCommand>),
}

pub struct Ui {
    pub app: App,
    pub composer: Composer,
    pub index: FileIndex,
    pub overlay: Option<Overlay>,
    pub popup: Option<Popup>,
    pub popup_sel: usize,
    pub message: Option<String>,
    pub show_thinking: bool,
    pub focus_approval: bool,
    pub maintenance_running: bool,
    pub working_dir: PathBuf,
    /// Change previews by tool call, for /diff.
    pub previews: HashMap<String, ChangePreview>,
    last_ctrl_c: Option<Instant>,
}

impl Ui {
    pub fn new(working_dir: &Path) -> Self {
        Self {
            app: App::new(),
            composer: Composer::new(),
            index: FileIndex::new(working_dir),
            overlay: None,
            popup: None,
            popup_sel: 0,
            message: None,
            show_thinking: false,
            focus_approval: false,
            maintenance_running: false,
            working_dir: working_dir.to_path_buf(),
            previews: HashMap::new(),
            last_ctrl_c: None,
        }
    }

    pub fn popup_labels(&self) -> Option<(Vec<String>, &'static str)> {
        match self.popup.as_ref()? {
            Popup::Mentions(entries) => Some((
                entries
                    .iter()
                    .map(|e| {
                        if e.is_dir {
                            format!("{}/", e.path)
                        } else {
                            e.path.clone()
                        }
                    })
                    .collect(),
                if self.index.truncated {
                    "files (index truncated) — Tab/Enter attach · Ctrl+R refresh"
                } else {
                    "files — Tab/Enter attach · Ctrl+R refresh · Esc close"
                },
            )),
            Popup::Commands(cmds) => Some((
                cmds.iter()
                    .map(|c| format!("/{} {} — {}", c.name, c.args, c.description))
                    .collect(),
                "commands — Tab complete · Enter run · Esc close",
            )),
        }
    }

    /// Recompute the completion list from the composer.
    pub fn refresh_popup(&mut self) {
        let before = self.popup.is_some();
        if let Some((_, q)) = self.composer.mention_query() {
            if !before {
                self.index.ensure_fresh();
            }
            let hits = self.index.search(&q, 50);
            self.popup = Some(Popup::Mentions(hits));
        } else if let Some(q) = self.composer.slash_query() {
            self.popup = Some(Popup::Commands(commands::complete(q)));
        } else {
            self.popup = None;
        }
        let n = self.popup_labels().map(|(l, _)| l.len()).unwrap_or(0);
        if n == 0 {
            self.popup_sel = 0;
        } else if self.popup_sel >= n {
            self.popup_sel = n - 1;
        }
    }

    pub fn on_event(&mut self, env: &Envelope) {
        self.app.apply(env);
        match &env.event {
            Event::ApprovalRequested { approval } => {
                if let Some(p) = &approval.preview {
                    self.previews
                        .insert(approval.tool_call_id.clone(), p.clone());
                }
                self.focus_approval = true;
                self.popup = None;
            }
            Event::ApprovalResolved { .. } | Event::RunFinished { .. } => {
                if self.app.pending.is_empty() {
                    self.focus_approval = false;
                }
            }
            Event::MemoryMaintenanceStarted => self.maintenance_running = true,
            Event::MemoryMaintenanceFinished { .. } => self.maintenance_running = false,
            _ => {}
        }
        if self.app.files_changed {
            self.index.invalidate();
            self.app.files_changed = false;
        }
    }

    pub fn on_paste(&mut self, text: &str) {
        if self.overlay.is_some() {
            return;
        }
        self.composer.insert_str(text);
        self.refresh_popup();
    }

    fn approve(&mut self, req: &ApprovalRequest, decision: Decision) -> Vec<Effect> {
        vec![Effect::Send(Command::Approve {
            approval_id: req.approval_id.clone(),
            decision,
            reason: (decision == Decision::Deny).then(|| "rejected in the interface".to_string()),
        })]
    }

    /// The details of a pending approval (diff or input).
    pub fn approval_overlay(req: &ApprovalRequest) -> Overlay {
        let mut lines: Vec<Line<'static>> = vec![
            Line::from(format!(
                "{} ({}) — {}",
                req.tool,
                req.level,
                req.description.lines().next().unwrap_or("")
            )),
            Line::default(),
        ];
        match &req.preview {
            Some(p) => {
                for f in &p.files {
                    lines.push(Line::from(format!(
                        "{} ({:?}, +{} −{})",
                        f.path, f.kind, f.added, f.removed
                    )));
                    lines.extend(f.diff.lines().map(crate::view::diff_line));
                    lines.push(Line::default());
                }
            }
            None => {
                lines.push(Line::from(
                    "No file preview: the effects of this call (a command, an MCP call) cannot be shown as a diff.",
                ));
                lines.push(Line::default());
                let input = serde_json::to_string_pretty(&req.input).unwrap_or_default();
                lines.extend(input.lines().map(|l| Line::from(l.to_string())));
                lines.extend(
                    req.description
                        .lines()
                        .skip(1)
                        .map(|l| Line::from(l.to_string())),
                );
            }
        }
        Overlay::text(
            "proposed change — y allow · a session · n reject from the input",
            lines,
        )
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return Vec::new();
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);

        // Ctrl+C: always reachable, whatever has the focus.
        if ctrl && key.code == KeyCode::Char('c') {
            if self.app.running() || self.maintenance_running {
                self.message = Some("cancelling…".into());
                return vec![Effect::Send(Command::Cancel)];
            }
            if let Some(o) = self.overlay.take() {
                drop(o);
                return Vec::new();
            }
            if !self.composer.is_empty() {
                self.composer.clear();
                self.popup = None;
                return Vec::new();
            }
            if self
                .last_ctrl_c
                .is_some_and(|t| t.elapsed() < Duration::from_millis(1500))
            {
                return vec![Effect::Quit];
            }
            self.last_ctrl_c = Some(Instant::now());
            self.message = Some("Ctrl+C again to quit (the session stays stored)".into());
            return Vec::new();
        }
        self.message = None;

        if self.overlay.is_some() {
            return self.overlay_key(key);
        }

        if self.focus_approval {
            if let Some(req) = self.app.pending.first().cloned() {
                match key.code {
                    KeyCode::Char('y') => return self.approve(&req, Decision::Allow),
                    KeyCode::Char('a') => return self.approve(&req, Decision::AllowForSession),
                    KeyCode::Char('n') => return self.approve(&req, Decision::Deny),
                    KeyCode::Char('d') | KeyCode::Enter => {
                        self.overlay = Some(Self::approval_overlay(&req));
                        return Vec::new();
                    }
                    KeyCode::Tab | KeyCode::Esc => {
                        self.focus_approval = false;
                        return Vec::new();
                    }
                    _ => return Vec::new(),
                }
            }
            self.focus_approval = false;
        }

        // Completion lists.
        if self.popup.is_some() {
            let n = self.popup_labels().map(|(l, _)| l.len()).unwrap_or(0);
            match key.code {
                KeyCode::Up => {
                    self.popup_sel = self.popup_sel.saturating_sub(1);
                    return Vec::new();
                }
                KeyCode::Down => {
                    if self.popup_sel + 1 < n {
                        self.popup_sel += 1;
                    }
                    return Vec::new();
                }
                KeyCode::Esc => {
                    self.popup = None;
                    return Vec::new();
                }
                KeyCode::Char('r') if ctrl => {
                    self.index.rebuild();
                    self.refresh_popup();
                    return Vec::new();
                }
                KeyCode::Tab | KeyCode::Enter if n > 0 => {
                    return self.accept_popup(key.code == KeyCode::Enter)
                }
                _ => {}
            }
        }

        match key.code {
            KeyCode::Enter if shift || alt => self.composer.newline(),
            KeyCode::Char('j') if ctrl => self.composer.newline(),
            KeyCode::Enter => {
                if self.composer.text().ends_with('\\') {
                    self.composer.backspace();
                    self.composer.newline();
                } else {
                    return self.submit();
                }
            }
            KeyCode::Tab => {
                if !self.app.pending.is_empty() {
                    self.focus_approval = true;
                }
            }
            KeyCode::Char('d') if ctrl => {
                if self.composer.is_empty() {
                    return vec![Effect::Quit];
                }
                self.composer.delete();
            }
            KeyCode::Char('t') if ctrl => self.show_thinking = !self.show_thinking,
            KeyCode::Char('o') if ctrl => self.overlay = Some(self.details_overlay()),
            KeyCode::Char('w') if ctrl => self.composer.delete_word_left(),
            KeyCode::Char('a') if ctrl => self.composer.home(false),
            KeyCode::Char('e') if ctrl => self.composer.end(false),
            KeyCode::Char('u') if ctrl => self.composer.clear(),
            KeyCode::Char(c) if !ctrl => self.composer.insert_str(&c.to_string()),
            KeyCode::Backspace => {
                if self.composer.text().is_empty()
                    || self.composer.cursor() == 0 && self.composer.selection().is_none()
                {
                    if self.composer.text().is_empty() {
                        self.composer.remove_last_attachment();
                    } else {
                        self.composer.backspace();
                    }
                } else if alt || ctrl {
                    self.composer.delete_word_left();
                } else {
                    self.composer.backspace();
                }
            }
            KeyCode::Delete => self.composer.delete(),
            KeyCode::Left if ctrl || alt => self.composer.word_left(shift),
            KeyCode::Right if ctrl || alt => self.composer.word_right(shift),
            KeyCode::Left => self.composer.left(shift),
            KeyCode::Right => self.composer.right(shift),
            KeyCode::Home => self.composer.home(shift),
            KeyCode::End => self.composer.end(shift),
            KeyCode::Up => {
                if self.composer.on_first_line() && !shift {
                    self.composer.history_prev();
                } else {
                    self.composer.up(shift);
                }
            }
            KeyCode::Down => {
                if self.composer.on_last_line() && !shift {
                    self.composer.history_next();
                } else {
                    self.composer.down(shift);
                }
            }
            KeyCode::Esc => self.popup = None,
            _ => {}
        }
        self.refresh_popup();
        Vec::new()
    }

    fn accept_popup(&mut self, enter: bool) -> Vec<Effect> {
        let sel = self.popup_sel;
        match self.popup.take() {
            Some(Popup::Mentions(entries)) => {
                if let Some(e) = entries.get(sel) {
                    match self.index.resolve(e) {
                        Ok(abs) => {
                            let is_image =
                                abs.extension().and_then(|x| x.to_str()).is_some_and(|x| {
                                    ["png", "jpg", "jpeg", "gif", "webp"]
                                        .contains(&x.to_ascii_lowercase().as_str())
                                });
                            let a = if e.is_dir {
                                Attachment::Folder(e.path.clone())
                            } else if is_image {
                                Attachment::Image(e.path.clone())
                            } else {
                                Attachment::File(e.path.clone())
                            };
                            self.composer.complete_mention(a);
                        }
                        Err(why) => {
                            self.message = Some(format!("{why} — the list was refreshed"));
                            self.index.rebuild();
                        }
                    }
                }
                self.refresh_popup();
                Vec::new()
            }
            Some(Popup::Commands(cmds)) => {
                let Some(c) = cmds.get(sel) else {
                    return Vec::new();
                };
                if enter && c.args.is_empty() {
                    self.composer.set_text(&format!("/{}", c.name));
                    return self.submit();
                }
                self.composer.set_text(&format!("/{} ", c.name));
                self.refresh_popup();
                Vec::new()
            }
            None => Vec::new(),
        }
    }

    /// Enter: a command, or a prompt.
    pub fn submit(&mut self) -> Vec<Effect> {
        let text = self.composer.text().trim().to_string();
        if text.starts_with('/') && self.composer.attachments().is_empty() {
            self.composer.clear();
            self.popup = None;
            return match commands::parse(&text) {
                Ok(Action::Engine(cmd)) => vec![Effect::Send(cmd)],
                Ok(Action::Present(Present::Quit)) => vec![Effect::Quit],
                Ok(Action::Present(Present::Attach(kind, path))) => {
                    self.attach_path(kind, &path);
                    Vec::new()
                }
                Ok(Action::Present(p)) => vec![Effect::Present(p)],
                Err(e) => {
                    self.message = Some(e);
                    Vec::new()
                }
            };
        }
        if self.composer.is_empty() {
            return Vec::new();
        }
        if self.app.running() {
            // Never started silently in parallel; the text stays editable.
            self.message = Some(
                "a run is in progress: wait for it or Ctrl+C to cancel; your text is kept".into(),
            );
            return Vec::new();
        }
        let prompt = self.composer.take();
        self.popup = None;
        vec![Effect::Send(Command::Submit { prompt })]
    }

    fn attach_path(&mut self, kind: AttachKind, path: &str) {
        let abs = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            self.working_dir.join(path)
        };
        let ok = match kind {
            AttachKind::Folder => abs.is_dir(),
            _ => abs.is_file(),
        };
        if !ok {
            self.message = Some(format!(
                "`{path}` does not exist (or is not a {})",
                match kind {
                    AttachKind::Folder => "folder",
                    _ => "file",
                }
            ));
            return;
        }
        self.composer.attach(match kind {
            AttachKind::File => Attachment::File(path.to_string()),
            AttachKind::Folder => Attachment::Folder(path.to_string()),
            AttachKind::Image => Attachment::Image(path.to_string()),
        });
    }

    fn overlay_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let Some(o) = self.overlay.as_mut() else {
            return Vec::new();
        };
        match (o, key.code) {
            (_, KeyCode::Esc) => self.overlay = None,
            (Overlay::Text { .. }, KeyCode::Char('q')) => self.overlay = None,
            (o, KeyCode::Up) => o.scroll_by(-1, 20),
            (o, KeyCode::Down) => o.scroll_by(1, 20),
            (o, KeyCode::PageUp) => o.scroll_by(-20, 20),
            (o, KeyCode::PageDown) => o.scroll_by(20, 20),
            (o, KeyCode::Home) => o.scroll_by(-1_000_000, 20),
            (o, KeyCode::End) => o.scroll_by(1_000_000, 20),
            (o @ Overlay::Picker { .. }, KeyCode::Enter) => {
                let index = match o {
                    Overlay::Picker { selected, .. } => *selected,
                    _ => 0,
                };
                let chosen = o.visible().get(index).map(|i| i.value.clone());
                let action = match o {
                    Overlay::Picker { action, .. } => action.clone(),
                    _ => PickAction::Session,
                };
                self.overlay = None;
                if let Some(value) = chosen {
                    return self.picked(action, value);
                }
            }
            (
                Overlay::Picker {
                    filter, selected, ..
                },
                KeyCode::Char(c),
            ) => {
                filter.push(c);
                *selected = 0;
            }
            (
                Overlay::Picker {
                    filter, selected, ..
                },
                KeyCode::Backspace,
            ) => {
                filter.pop();
                *selected = 0;
            }
            _ => {}
        }
        Vec::new()
    }

    /// A picker choice.
    fn picked(&mut self, action: PickAction, value: Option<String>) -> Vec<Effect> {
        match action {
            PickAction::Model => match value {
                Some(model) => vec![Effect::ChooseProfile(model)],
                None => Vec::new(),
            },
            PickAction::Profile { model } => vec![Effect::Send(Command::SetModel {
                model: Some(model),
                reasoning: value,
            })],
            PickAction::Session => match value {
                Some(id) => vec![Effect::Send(Command::Resume { session_id: id })],
                None => Vec::new(),
            },
        }
    }

    /// Details of the last tool calls and reasoning (Ctrl+O).
    pub fn details_overlay(&self) -> Overlay {
        let mut lines: Vec<Line<'static>> = Vec::new();
        let mut found = false;
        for c in self.app.cells.iter().rev() {
            match c {
                Cell::Tools { calls } if !found => {
                    found = true;
                    for t in calls {
                        lines.push(Line::from(format!(
                            "── {} {} ({:?}, {} ms)",
                            t.name, t.summary, t.status, t.duration_ms
                        )));
                        lines.extend(t.output.lines().map(|l| Line::from(l.to_string())));
                        if t.output_bytes > t.output.len() {
                            lines.push(Line::from(format!(
                                "… {} more bytes (the full output is stored with the session)",
                                t.output_bytes - t.output.len()
                            )));
                        }
                        lines.push(Line::default());
                    }
                }
                Cell::Thinking { text, .. } => {
                    lines.push(Line::from("── reasoning (as exposed by the provider)"));
                    lines.extend(text.lines().map(|l| Line::from(l.to_string())));
                    lines.push(Line::default());
                    break;
                }
                Cell::User { .. } if found => break,
                _ => {}
            }
        }
        if lines.is_empty() {
            lines.push(Line::from("No tool call or exposed reasoning yet."));
        }
        Overlay::text("details", lines)
    }

    /// Model picker items.
    pub fn model_picker(models: &[cersei_agent::control::ModelChoice], current: &str) -> Overlay {
        let items = models
            .iter()
            .map(|m| PickItem {
                label: format!(
                    "{}{}",
                    if m.selection == current { "● " } else { "  " },
                    m.selection
                ),
                detail: format!(
                    "{} · {} input tokens{}{}",
                    m.name,
                    m.max_input_tokens,
                    if m.profiles.is_empty() {
                        String::new()
                    } else {
                        format!(
                            " · profiles: {}",
                            m.profiles
                                .iter()
                                .map(|p| p.id.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    },
                    if m.priced {
                        ""
                    } else {
                        " · no price (cost unknown)"
                    }
                ),
                value: Some(m.selection.clone()),
            })
            .collect();
        Overlay::Picker {
            title: "model (from the configuration)".into(),
            items,
            filter: String::new(),
            selected: 0,
            action: PickAction::Model,
        }
    }

    /// Profile picker for a model.
    pub fn profile_picker(model: &cersei_agent::control::ModelChoice) -> Overlay {
        let mut items = vec![PickItem {
            label: "(default)".into(),
            detail: model
                .default_profile
                .clone()
                .map(|d| format!("the model's default: {d}"))
                .unwrap_or_else(|| "no profile parameters".into()),
            value: None,
        }];
        items.extend(model.profiles.iter().map(|p| PickItem {
            label: p.id.clone(),
            detail: p.label.clone(),
            value: Some(p.id.clone()),
        }));
        Overlay::Picker {
            title: format!("reasoning profile of {}", model.selection),
            items,
            filter: String::new(),
            selected: 0,
            action: PickAction::Profile {
                model: model.selection.clone(),
            },
        }
    }
}
