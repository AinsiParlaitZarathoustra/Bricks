//! The attachments of the messages a session was sent with, kept so that the
//! conversation shows them again after a switch or a restart.
//!
//! The engine keeps the text of a message and, for an image, its bytes, but
//! not the copy's path or name. The desktop therefore keeps one record per run
//! that started with copies, in `<session files>/attachments.jsonl` (one JSON
//! line per run, oldest first). The file lives in the session's folder, so
//! deleting the session deletes it with the copies. A record holds the run's
//! prompt and the number of user messages before the run, and finds its message
//! again at reading time, the way run labels do: the first user message from
//! that count whose text starts with the prompt. A record whose message is not
//! in the history is ignored: nothing is placed on a message it does not belong
//! to. Records of an older session do not exist: its messages show no
//! attachment, as before.

use super::attachments::{copy_path, session_attachments};
use super::conversations::message_text;
use super::io_error;
use crate::contract::{AttachmentRef, HistoryBlock, HistoryMessage, IpcErrorCode, IpcResult};
use bricks_types::{Message, Role};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::Path;

/// The file of the records, in a session's folder.
const RUNS_FILE: &str = "attachments.jsonl";

/// Messages the engine may place before a run's prompt (the skill activations
/// of the run). The prompt is within them.
const PROMPT_WINDOW: usize = 4;

/// The copies a run was started with.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunAttachments {
    pub run_id: String,
    /// The user-role messages before the run.
    pub base: u32,
    /// The text of the run's prompt, as the engine received it.
    pub prompt: String,
    /// The copies, in the order of the prompt's blocks.
    pub refs: Vec<AttachmentRef>,
}

/// Appends a record to the session's folder. Nothing is written when the
/// folder is gone (the session was deleted meanwhile).
pub fn append(files: &Path, record: &RunAttachments) -> IpcResult<()> {
    if !files.is_dir() {
        return Ok(());
    }
    let line = serde_json::to_string(record)
        .map_err(|e| crate::services::error(IpcErrorCode::Internal, e.to_string()))?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(files.join(RUNS_FILE))
        .map_err(|e| io_error("pièces jointes des messages", e))?;
    file.write_all(format!("{line}\n").as_bytes())
        .and_then(|()| file.sync_data())
        .map_err(|e| io_error("pièces jointes des messages", e))
}

/// Sets aside the records of the runs of the messages from `user_index` on (a
/// revision replaced them), in `attachments.revised.jsonl`. The copies stay.
pub fn set_aside_from(files: &Path, user_index: u32) -> IpcResult<usize> {
    super::run_labels::move_lines(
        &files.join(RUNS_FILE),
        &files.join("attachments.revised.jsonl"),
        |line| serde_json::from_str::<RunAttachments>(line).is_ok_and(|r| r.base >= user_index),
    )
    .map_err(|e| io_error("pièces jointes des messages", e))
}

/// The records of a session, oldest first. A missing file gives none; a damaged
/// line is skipped, never fatal.
pub fn load(files: &Path) -> Vec<RunAttachments> {
    let mut text = String::new();
    match std::fs::File::open(files.join(RUNS_FILE)) {
        Ok(mut file) => {
            if file.read_to_string(&mut text).is_err() {
                return Vec::new();
            }
        }
        Err(_) => return Vec::new(),
    }
    let mut skipped = 0usize;
    let records: Vec<RunAttachments> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| match serde_json::from_str(l) {
            Ok(record) => Some(record),
            Err(_) => {
                skipped += 1;
                None
            }
        })
        .collect();
    if skipped > 0 {
        eprintln!(
            "bricks-desktop: {skipped} damaged line(s) skipped in {}",
            files.join(RUNS_FILE).display()
        );
    }
    records
}

/// The ids of the copies a record of the session refers to: a copy referred to
/// by a sent message is kept.
pub fn referenced(files: &Path) -> HashSet<String> {
    load(files)
        .into_iter()
        .flat_map(|r| r.refs.into_iter().map(|a| a.id))
        .collect()
}

/// Gives each user message of `history` the copies its run was started with
/// (`HistoryMessage::attachments`), and takes out the engine's wrappers of
/// those copies (`<attached_file path="…">`): the copy is shown as an
/// attachment, not as the raw text of the file. A copy that is no longer on
/// disk is not given. `messages` and `history` are the same messages, in order.
pub fn link_history(
    messages: &[Message],
    history: &mut [HistoryMessage],
    records: &[RunAttachments],
    files: &Path,
) {
    let dir = session_attachments(files);
    for record in records {
        let Some(index) = message_index(messages, record.base, &record.prompt) else {
            continue;
        };
        let Some(message) = history.get_mut(index) else {
            continue;
        };
        let mut paths = Vec::new();
        for attachment in &record.refs {
            let path = copy_path(&dir, &attachment.id, &attachment.name);
            if !path.is_file() || message.attachments.iter().any(|a| a.id == attachment.id) {
                continue;
            }
            paths.push(path.display().to_string());
            message.attachments.push(attachment.clone());
        }
        if !paths.is_empty() {
            message.blocks.retain(|block| !is_wrapper_of(block, &paths));
        }
    }
}

/// The index, in `messages`, of the user message a run added: the first user
/// message from `base` (within [`PROMPT_WINDOW`]) whose text starts with the
/// prompt. The copy wrappers that follow the prompt do not stop the match. A
/// blank prompt is not stored as a text block, so it matches the first message
/// of the window.
pub fn message_index(messages: &[Message], base: u32, prompt: &str) -> Option<usize> {
    let blank = prompt.trim().is_empty();
    messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role == Role::User)
        .skip(base as usize)
        .take(PROMPT_WINDOW)
        .find(|(_, m)| blank || message_text(m).starts_with(prompt))
        .map(|(i, _)| i)
}

/// Whether a block is the engine's wrapper of one of the copies at `paths`.
fn is_wrapper_of(block: &HistoryBlock, paths: &[String]) -> bool {
    match block {
        HistoryBlock::Text { text } => paths
            .iter()
            .any(|p| text.starts_with(&format!("<attached_file path=\"{p}\""))),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{AttachmentKind, HistoryRole};
    use bricks_types::ContentBlock;

    fn aref(id: &str, name: &str) -> AttachmentRef {
        AttachmentRef {
            id: id.into(),
            name: name.into(),
            kind: AttachmentKind::Text,
            mime: "text/plain".into(),
            size: 3,
        }
    }

    fn user(text: &str) -> Message {
        Message::user(text)
    }

    #[test]
    fn records_are_appended_and_read_back_in_order() {
        let root = tempfile::tempdir().unwrap();
        let files = root.path().join("s.files");
        // Without the folder (deleted session), nothing is written.
        let record = RunAttachments {
            run_id: "r1".into(),
            base: 0,
            prompt: "Regarde".into(),
            refs: vec![aref(&"a".repeat(32), "a.txt")],
        };
        append(&files, &record).unwrap();
        assert!(!files.exists());
        std::fs::create_dir_all(&files).unwrap();
        append(&files, &record).unwrap();
        append(
            &files,
            &RunAttachments {
                run_id: "r2".into(),
                ..record.clone()
            },
        )
        .unwrap();
        let loaded = load(&files);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[1].run_id, "r2");
        assert_eq!(loaded[0].refs[0].name, "a.txt");
        assert!(referenced(&files).contains(&"a".repeat(32)));
    }

    #[test]
    fn a_damaged_line_is_skipped() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join(RUNS_FILE),
            "pas du json\n{\"run_id\":\"x\",\"base\":0,\"prompt\":\"\",\"refs\":[]}\n",
        )
        .unwrap();
        assert_eq!(load(root.path()).len(), 1);
    }

    #[test]
    fn the_message_of_a_run_is_found_after_its_base_and_past_a_skill_message() {
        let messages = vec![
            user("premier"),
            Message::assistant("réponse"),
            user("Regarde ce fichier"),
        ];
        assert_eq!(message_index(&messages, 1, "Regarde"), Some(2));
        // A prompt that is not there gives nothing (nothing is placed on another message).
        assert_eq!(message_index(&messages, 1, "autre"), None);
        // A skill activation before the prompt, within the window.
        let with_skill = vec![user("premier"), user("<skill>"), user("Regarde")];
        assert_eq!(message_index(&with_skill, 1, "Regarde"), Some(2));
    }

    #[test]
    fn a_user_message_gets_its_copies_and_the_wrapper_of_each_copy_is_hidden() {
        let root = tempfile::tempdir().unwrap();
        let files = root.path().join("s.files");
        let dir = session_attachments(&files);
        std::fs::create_dir_all(&dir).unwrap();
        let id = "b".repeat(32);
        let path = copy_path(&dir, &id, "notes.txt");
        std::fs::write(&path, "abc").unwrap();
        let wrapper = format!(
            "<attached_file path=\"{}\" bytes=\"3\">\nabc\n</attached_file>",
            path.display()
        );
        // The engine's message: the prompt, then the wrapper of the copy.
        let messages = vec![Message::user_blocks(vec![
            ContentBlock::Text {
                text: "Regarde".into(),
            },
            ContentBlock::Text {
                text: wrapper.clone(),
            },
        ])];
        // The desktop's view of the same message.
        let mut history = vec![HistoryMessage {
            role: HistoryRole::User,
            blocks: vec![
                HistoryBlock::Text {
                    text: "Regarde".into(),
                },
                HistoryBlock::Text { text: wrapper },
            ],
            attachments: Vec::new(),
        }];
        let records = vec![RunAttachments {
            run_id: "r".into(),
            base: 0,
            prompt: "Regarde".into(),
            refs: vec![aref(&id, "notes.txt"), aref(&"c".repeat(32), "gone.txt")],
        }];
        link_history(&messages, &mut history, &records, &files);
        assert_eq!(
            history[0].attachments.len(),
            1,
            "a copy that is gone is not given"
        );
        assert_eq!(history[0].attachments[0].id, id);
        assert_eq!(
            history[0].blocks.len(),
            1,
            "the wrapper of the copy is hidden"
        );
    }
}

#[cfg(test)]
mod revision_tests {
    use super::*;

    fn run(id: &str, base: u32, copy: &str) -> RunAttachments {
        RunAttachments {
            run_id: id.into(),
            base,
            prompt: format!("prompt {id}"),
            refs: vec![AttachmentRef {
                id: copy.repeat(32),
                name: format!("{id}.txt"),
                kind: crate::contract::AttachmentKind::Text,
                mime: "text/plain".into(),
                size: 1,
            }],
        }
    }

    #[test]
    fn a_revision_sets_aside_the_records_from_its_message_on_and_keeps_the_copies_of_the_others() {
        let root = tempfile::tempdir().unwrap();
        let files = root.path().join("s.files");
        std::fs::create_dir_all(&files).unwrap();
        append(&files, &run("r0", 0, "a")).unwrap();
        append(&files, &run("r1", 1, "b")).unwrap();
        append(&files, &run("r2", 2, "c")).unwrap();

        assert_eq!(set_aside_from(&files, 1).unwrap(), 2);

        let kept = load(&files);
        assert_eq!(kept.iter().map(|r| r.run_id.as_str()).collect::<Vec<_>>(), ["r0"]);
        assert!(referenced(&files).contains(&"a".repeat(32)));
        let aside = std::fs::read_to_string(files.join("attachments.revised.jsonl")).unwrap();
        assert_eq!(aside.lines().count(), 2);
    }

    #[test]
    fn a_folder_that_is_gone_moves_nothing_and_writes_nothing() {
        let root = tempfile::tempdir().unwrap();
        let files = root.path().join("absent.files");
        assert_eq!(set_aside_from(&files, 0).unwrap(), 0);
        assert!(!files.exists());
    }
}
