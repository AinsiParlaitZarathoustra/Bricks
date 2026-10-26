//! Presentation labels of runs started from typed commands.
//!
//! The engine receives a plain `submit` for `/skill debug …`; the desktop
//! remembers how the user typed it. Labels live in
//! `<bricks_home>/desktop/run-labels/<session_id>.jsonl`: one [`RunLabel`]
//! per line, append-only, oldest first, one per run (deduplicated by
//! `run_id`). A damaged line is skipped on read and never fatal.
//!
//! A deleted session's labels file is removed and the session id is then
//! refused for good by this process, so a run still draining its events
//! cannot recreate the file.

use super::conversations::message_text;
use super::{error, invalid, io_error};
use crate::contract::{IpcErrorCode, IpcResult, RunLabel};
use bricks_types::{Message, Role};
use parking_lot::Mutex;
use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Longest label kept, in characters (a typed command, not a prompt).
pub const MAX_LABEL_CHARS: usize = 200;

/// Messages the engine may place before a run's prompt: the skill activation
/// of the run and a skill text restored at load. The prompt is within them.
const PROMPT_WINDOW: usize = 4;

/// How many user-role messages the history holds: the index space of
/// [`RunLabel::user_index`].
pub fn user_count(history: &[Message]) -> u32 {
    let n = history.iter().filter(|m| m.role == Role::User).count();
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// The index, among the user-role messages of `history`, of the message a run
/// added: the first one from `base` (the user messages before the run) whose
/// text is `prompt`. Only the few messages the engine puts before the prompt
/// are looked at, so an identical message earlier in the session is never
/// taken for it. `None` when the prompt is not there (the run did not reach
/// it, or the history changed underneath).
pub fn user_index_of(history: &[Message], base: u32, prompt: &str) -> Option<u32> {
    history
        .iter()
        .filter(|m| m.role == Role::User)
        .enumerate()
        .skip(base as usize)
        .take(PROMPT_WINDOW)
        .find(|(_, m)| message_text(m) == prompt)
        .map(|(i, _)| u32::try_from(i).unwrap_or(u32::MAX))
}

pub struct RunLabelStore {
    dir: PathBuf,
    /// Serialises appends and deletions; session ids deleted by this
    /// process, which no later label may recreate.
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    deleted: HashSet<String>,
}

impl RunLabelStore {
    pub fn at(dir: PathBuf) -> Self {
        Self {
            dir,
            state: Mutex::new(State::default()),
        }
    }

    /// The labels of a session, oldest first. A missing file (an older
    /// session) gives none; a damaged line is skipped.
    pub fn list(&self, session_id: &str) -> IpcResult<Vec<RunLabel>> {
        check_session_id(session_id)?;
        self.read(&self.path(session_id))
    }

    /// Appends the label of a run, unless the run already has one or the
    /// session was deleted. Returns whether it was appended. Blocking I/O:
    /// call from `spawn_blocking`.
    pub fn append(&self, session_id: &str, label: &RunLabel) -> IpcResult<bool> {
        check_session_id(session_id)?;
        let state = self.state.lock();
        if state.deleted.contains(session_id) {
            return Ok(false);
        }
        let path = self.path(session_id);
        let existing = self.read(&path)?;
        if existing.iter().any(|l| l.run_id == label.run_id) {
            return Ok(false);
        }
        let line = serde_json::to_string(label)
            .map_err(|e| error(IpcErrorCode::Internal, e.to_string()))?;
        append_line(&path, &line).map_err(|e| io_error("étiquettes des commandes", e))?;
        Ok(true)
    }

    /// Removes the labels of a deleted session, and refuses later labels for
    /// it. Blocking I/O: call from `spawn_blocking`.
    pub fn delete(&self, session_id: &str) -> IpcResult<()> {
        check_session_id(session_id)?;
        let mut state = self.state.lock();
        state.deleted.insert(session_id.to_string());
        // What earlier revisions set aside goes with the session.
        let aside = self.dir.join(format!("{session_id}.revised.jsonl"));
        if let Err(e) = std::fs::remove_file(&aside)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            return Err(io_error("étiquettes des commandes", e));
        }
        match std::fs::remove_file(self.path(session_id)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_error("étiquettes des commandes", e)),
        }
    }

    /// Sets aside the labels of the messages from `user_index` on (a
    /// revision replaced them), in `<session_id>.revised.jsonl`. Blocking
    /// I/O: call from `spawn_blocking`.
    pub fn set_aside_from(&self, session_id: &str, user_index: u32) -> IpcResult<usize> {
        check_session_id(session_id)?;
        let state = self.state.lock();
        if state.deleted.contains(session_id) {
            return Ok(0);
        }
        move_lines(
            &self.path(session_id),
            &self.dir.join(format!("{session_id}.revised.jsonl")),
            |line| {
                serde_json::from_str::<RunLabel>(line)
                    .is_ok_and(|l| l.user_index.is_some_and(|i| i >= user_index))
            },
        )
        .map_err(|e| io_error("étiquettes des commandes", e))
    }

    /// Whether the session has a labels file (for the deletion check).
    pub fn exists(&self, session_id: &str) -> bool {
        check_session_id(session_id).is_ok() && self.path(session_id).exists()
    }

    fn path(&self, session_id: &str) -> PathBuf {
        self.dir.join(format!("{session_id}.jsonl"))
    }

    fn read(&self, path: &Path) -> IpcResult<Vec<RunLabel>> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io_error("étiquettes des commandes", e)),
        };
        let mut skipped = 0usize;
        let labels: Vec<RunLabel> = text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| match serde_json::from_str(l) {
                Ok(label) => Some(label),
                Err(_) => {
                    skipped += 1;
                    None
                }
            })
            .collect();
        if skipped > 0 {
            eprintln!(
                "bricks-desktop: {skipped} damaged line(s) skipped in {}",
                path.display()
            );
        }
        Ok(labels)
    }
}

/// The label as the person typed it, for display: trimmed, empty is none,
/// at most [`MAX_LABEL_CHARS`] characters.
pub fn normalize_label(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(MAX_LABEL_CHARS).collect())
}

/// A session id becomes a file name: letters, digits, `-`, `_` and `.`, not
/// starting with a dot (the same rule as the engine's session ids).
pub(crate) fn check_session_id(id: &str) -> IpcResult<()> {
    let ok = !id.is_empty()
        && id.len() <= 128
        && !id.starts_with('.')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if ok {
        Ok(())
    } else {
        Err(invalid("identifiant de conversation invalide"))
    }
}

/// Moves the lines of a JSON-lines file that `moves` accepts to the end of
/// `aside` (kept, so a revision can be looked at again); the others stay, and
/// the file is rewritten atomically. Moved lines are written aside first, so
/// an interruption keeps them in both files, never in neither. Returns how
/// many lines moved; a missing file moves none.
pub(crate) fn move_lines(path: &Path, aside: &Path, moves: impl Fn(&str) -> bool) -> std::io::Result<usize> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    let (moved, kept): (Vec<&str>, Vec<&str>) = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .partition(|l| moves(l));
    if moved.is_empty() {
        return Ok(0);
    }
    for line in &moved {
        append_line(aside, line)?;
    }
    let mut rest = kept.join("\n");
    if !rest.is_empty() {
        rest.push('\n');
    }
    super::paths::write_atomic(path, rest.as_bytes())?;
    Ok(moved.len())
}

/// Appends one line, starting a new line first when the file does not end
/// with one (a write cut short by a crash must not spoil the next label).
pub(crate) fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut prefix = "";
    if let Ok(mut existing) = std::fs::File::open(path) {
        let len = existing.metadata()?.len();
        if len > 0 {
            existing.seek(SeekFrom::End(-1))?;
            let mut last = [0u8; 1];
            existing.read_exact(&mut last)?;
            if last[0] != b'\n' {
                prefix = "\n";
            }
        }
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(format!("{prefix}{line}\n").as_bytes())?;
    file.sync_data()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(run: &str, text: &str, at: i64) -> RunLabel {
        RunLabel {
            run_id: run.into(),
            label: text.into(),
            prompt: format!("prompt of {run}"),
            at,
            user_index: None,
        }
    }

    fn user(text: &str) -> Message {
        Message::user(text)
    }

    #[test]
    fn a_label_written_before_user_index_existed_reads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunLabelStore::at(dir.path().join("run-labels"));
        std::fs::create_dir_all(dir.path().join("run-labels")).unwrap();
        // A line as an older desktop wrote it: no `user_index` key.
        let old = r#"{"run_id":"run_a","label":"/skill debug","prompt":"corrige","at":1}"#;
        std::fs::write(dir.path().join("run-labels").join("s1.jsonl"), format!("{old}\n")).unwrap();
        let listed = store.list("s1").unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].user_index, None);
    }

    #[test]
    fn a_stored_user_index_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunLabelStore::at(dir.path().join("run-labels"));
        let mut indexed = label("run_a", "/skill debug", 1);
        indexed.user_index = Some(3);
        store.append("s1", &indexed).unwrap();
        assert_eq!(store.list("s1").unwrap(), vec![indexed]);
    }

    #[test]
    fn the_prompt_is_found_from_the_run_base_not_an_identical_earlier_message() {
        // Two identical plain messages, then the run's prompt, identical again.
        let history = vec![user("même"), Message::user("réponse"), user("même")];
        assert_eq!(user_index_of(&history, 0, "même"), Some(0), "from the start");
        assert_eq!(user_index_of(&history, 1, "même"), Some(2), "from the run's base");
        assert_eq!(user_index_of(&history, 3, "même"), None, "nothing added: no match");
        assert_eq!(user_count(&history), 3);
    }

    #[test]
    fn a_skill_activation_before_the_prompt_is_looked_past() {
        // Engine order: the activation (skill text) is pushed, then the prompt.
        let history = vec![
            user("premier"),
            user("Body of the skill debug, with its instructions"),
            user("Corrige ce problème"),
        ];
        assert_eq!(user_index_of(&history, 1, "Corrige ce problème"), Some(2));
        assert_eq!(user_index_of(&history, 1, "premier"), None, "earlier than the base");
    }

    #[test]
    fn a_prompt_that_is_not_in_the_history_gives_no_index() {
        let history = vec![user("autre")];
        assert_eq!(user_index_of(&history, 0, "absent"), None);
        assert_eq!(user_count(&[]), 0);
    }

    #[test]
    fn labels_are_listed_oldest_first_and_one_per_run() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunLabelStore::at(dir.path().join("run-labels"));
        assert!(store.list("s1").unwrap().is_empty(), "a missing file is no label");
        assert!(store.append("s1", &label("run_a", "/skill debug", 1)).unwrap());
        assert!(store.append("s1", &label("run_b", "/skill review", 2)).unwrap());
        assert!(!store.append("s1", &label("run_a", "/skill debug", 3)).unwrap(), "deduplicated");
        let listed = store.list("s1").unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].run_id, "run_a");
        assert_eq!(listed[1].label, "/skill review");
        assert!(store.list("s2").unwrap().is_empty(), "other sessions are apart");
    }

    #[test]
    fn a_damaged_line_is_skipped_and_the_next_label_starts_on_a_new_line() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunLabelStore::at(dir.path().join("run-labels"));
        store.append("s1", &label("run_a", "/skill debug", 1)).unwrap();
        let file = dir.path().join("run-labels").join("s1.jsonl");
        // A write cut short: a broken line without its newline at the end.
        let mut text = std::fs::read_to_string(&file).unwrap();
        text.push_str("{\"run_id\":\"run_x\",\"lab");
        std::fs::write(&file, text).unwrap();

        let listed = store.list("s1").unwrap();
        assert_eq!(listed.len(), 1, "the damaged line is skipped");
        assert_eq!(listed[0].run_id, "run_a");

        assert!(store.append("s1", &label("run_b", "/skill review", 2)).unwrap());
        let listed = store.list("s1").unwrap();
        assert_eq!(listed.iter().map(|l| l.run_id.as_str()).collect::<Vec<_>>(), ["run_a", "run_b"]);
    }

    #[test]
    fn a_deleted_session_loses_its_labels_and_gets_no_new_one() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunLabelStore::at(dir.path().join("run-labels"));
        store.append("s1", &label("run_a", "/skill debug", 1)).unwrap();
        assert!(store.exists("s1"));
        store.delete("s1").unwrap();
        assert!(!store.exists("s1"));
        assert!(!store.append("s1", &label("run_late", "/skill debug", 2)).unwrap());
        assert!(!store.exists("s1"), "a late label does not recreate the file");
        store.delete("s1").unwrap();
    }

    #[test]
    fn session_ids_are_file_names_only() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunLabelStore::at(dir.path().join("run-labels"));
        assert!(store.list("../escape").is_err());
        assert!(store.list(".hidden").is_err());
        assert!(store.list("").is_err());
        assert!(store.delete("a/b").is_err());
    }

    #[test]
    fn labels_are_trimmed_capped_and_empty_ones_are_none() {
        assert_eq!(normalize_label("  /skill debug  ").as_deref(), Some("/skill debug"));
        assert_eq!(normalize_label("   "), None);
        let long = "é".repeat(MAX_LABEL_CHARS + 10);
        assert_eq!(normalize_label(&long).unwrap().chars().count(), MAX_LABEL_CHARS);
    }
}

#[cfg(test)]
mod revision_tests {
    use super::*;

    fn indexed(run: &str, index: Option<u32>) -> RunLabel {
        RunLabel {
            run_id: run.into(),
            label: "/skill debug".into(),
            prompt: format!("prompt of {run}"),
            at: 1,
            user_index: index,
        }
    }

    fn aside_of(root: &Path) -> PathBuf {
        root.join("run-labels").join("s1.revised.jsonl")
    }

    #[test]
    fn a_revision_sets_aside_the_labels_from_its_message_on_and_keeps_the_others() {
        let root = tempfile::tempdir().unwrap();
        let store = RunLabelStore::at(root.path().join("run-labels"));
        let first = indexed("run_a", Some(0));
        let second = indexed("run_b", Some(1));
        let third = indexed("run_c", Some(2));
        let unindexed = indexed("run_old", None);
        for label in [&first, &second, &third, &unindexed] {
            store.append("s1", label).unwrap();
        }

        assert_eq!(store.set_aside_from("s1", 1).unwrap(), 2);

        assert_eq!(store.list("s1").unwrap(), vec![first, unindexed]);
        let aside = std::fs::read_to_string(aside_of(root.path())).unwrap();
        assert_eq!(aside.lines().count(), 2);
        assert!(aside.contains("run_b") && aside.contains("run_c"), "{aside}");
    }

    #[test]
    fn setting_aside_twice_moves_nothing_the_second_time() {
        let root = tempfile::tempdir().unwrap();
        let store = RunLabelStore::at(root.path().join("run-labels"));
        store.append("s1", &indexed("run_a", Some(0))).unwrap();
        store.append("s1", &indexed("run_b", Some(1))).unwrap();

        assert_eq!(store.set_aside_from("s1", 1).unwrap(), 1);
        assert_eq!(store.set_aside_from("s1", 1).unwrap(), 0);
        assert_eq!(store.list("s1").unwrap().len(), 1);
        assert_eq!(std::fs::read_to_string(aside_of(root.path())).unwrap().lines().count(), 1);
    }

    #[test]
    fn a_session_without_labels_file_moves_none_and_writes_no_aside_file() {
        let root = tempfile::tempdir().unwrap();
        let store = RunLabelStore::at(root.path().join("run-labels"));
        assert_eq!(store.set_aside_from("s1", 0).unwrap(), 0);
        assert!(!aside_of(root.path()).exists());
    }

    #[test]
    fn a_damaged_line_stays_where_it_is() {
        let root = tempfile::tempdir().unwrap();
        let store = RunLabelStore::at(root.path().join("run-labels"));
        store.append("s1", &indexed("run_a", Some(3))).unwrap();
        let file = root.path().join("run-labels").join("s1.jsonl");
        let mut text = std::fs::read_to_string(&file).unwrap();
        text.push_str("pas du json\n");
        std::fs::write(&file, text).unwrap();

        assert_eq!(store.set_aside_from("s1", 0).unwrap(), 1);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "pas du json\n");
    }

    #[test]
    fn nothing_is_set_aside_once_the_session_is_deleted() {
        let root = tempfile::tempdir().unwrap();
        let store = RunLabelStore::at(root.path().join("run-labels"));
        store.append("s1", &indexed("run_a", Some(0))).unwrap();
        store.append("s1", &indexed("run_b", Some(1))).unwrap();
        assert_eq!(store.set_aside_from("s1", 1).unwrap(), 1);
        assert!(aside_of(root.path()).exists());
        store.delete("s1").unwrap();
        assert_eq!(store.set_aside_from("s1", 0).unwrap(), 0);
        assert!(!aside_of(root.path()).exists(), "deleting the session removes its set-aside labels");
        assert!(!root.path().join("run-labels").join("s1.jsonl").exists());
    }

    #[test]
    fn move_lines_of_a_missing_file_is_none_and_keeps_the_aside_file_as_it_was() {
        let root = tempfile::tempdir().unwrap();
        let aside = root.path().join("aside.jsonl");
        std::fs::write(&aside, "déjà là\n").unwrap();
        let moved = move_lines(&root.path().join("absent.jsonl"), &aside, |_| true).unwrap();
        assert_eq!(moved, 0);
        assert_eq!(std::fs::read_to_string(&aside).unwrap(), "déjà là\n");
    }
}
