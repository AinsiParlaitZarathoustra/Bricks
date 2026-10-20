//! The change journal: the file changes Bricks applied, per project.
//!
//! `<bricks_home>/desktop/changes/<project_id>.jsonl` holds one
//! [`ChangeRecord`] per applied change (append-only). A record's stored
//! patches, when the approval preview had them, are in
//! `changes/<project_id>/<record_id>.json`. Records are deduplicated by
//! (session, sub-agent, tool call), so a change reported twice is kept once.

use super::paths::{desktop_dir, write_atomic};
use super::{error, io_error, invalid, not_found};
use crate::contract::{ChangeJournalInfo, ChangeRecord, FileDiff, IpcErrorCode, IpcResult};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Largest patch kept for one file (the rest is cut, and marked truncated).
pub const MAX_PATCH_BYTES: usize = 512 * 1024;

/// What the journal does not attribute, shown with the list.
pub const COVERAGE_NOTE: &str = "Enregistrées : les modifications de fichiers appliquées par \
    Bricks (outils d'écriture et d'édition, changements de sous-agents appliqués). Non \
    attribuées : les effets des commandes shell, des outils MCP et des modifications faites \
    hors de Bricks.";

/// A patch stored with a record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredPatch {
    pub path: String,
    pub patch: String,
    pub binary: bool,
    pub truncated: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct PatchFile {
    files: Vec<StoredPatch>,
}

/// A change to append: its record and the patches to keep with it.
pub struct NewChange {
    pub record: ChangeRecord,
    pub patches: Vec<StoredPatch>,
}

/// Identity of a change within the journal.
type Key = (String, Option<String>, String);

pub struct ChangeJournal {
    dir: PathBuf,
    /// Dedupe keys of each project's journal, read from the file once.
    keys: parking_lot::Mutex<HashMap<String, HashSet<Key>>>,
}

impl ChangeJournal {
    pub fn open() -> Self {
        Self::at(desktop_dir().join("changes"))
    }

    pub fn at(dir: PathBuf) -> Self {
        Self {
            dir,
            keys: parking_lot::Mutex::new(HashMap::new()),
        }
    }

    /// Appends `change` unless the same change is already recorded. Returns
    /// whether it was appended. Blocking I/O: call from `spawn_blocking`.
    pub fn append(&self, change: NewChange) -> IpcResult<bool> {
        let project = change.record.project_id.clone();
        check_id(&project)?;
        let mut keys = self.keys.lock();
        if !keys.contains_key(&project) {
            let existing = self
                .read_records(&project)?
                .iter()
                .map(key_of)
                .collect::<HashSet<Key>>();
            keys.insert(project.clone(), existing);
        }
        let set = keys.get_mut(&project).expect("inserted above");
        let key = key_of(&change.record);
        if set.contains(&key) {
            return Ok(false);
        }
        if !change.patches.is_empty() {
            let file = PatchFile {
                files: change.patches.clone(),
            };
            let bytes = serde_json::to_vec(&file)
                .map_err(|e| error(IpcErrorCode::Internal, e.to_string()))?;
            write_atomic(&self.patch_path(&project, &change.record.id), &bytes).map_err(|e| {
                io_error("journal des modifications (correctifs)", e)
            })?;
        }
        let line = serde_json::to_string(&change.record)
            .map_err(|e| error(IpcErrorCode::Internal, e.to_string()))?;
        append_line(&self.journal_path(&project), &line)
            .map_err(|e| io_error("journal des modifications", e))?;
        set.insert(key);
        Ok(true)
    }

    /// Records of a project, newest first; only those of `session_id` when
    /// given.
    pub fn list(
        &self,
        project_id: &str,
        session_id: Option<&str>,
        limit: usize,
    ) -> IpcResult<Vec<ChangeRecord>> {
        check_id(project_id)?;
        let mut records = self.read_records(project_id)?;
        records.retain(|r| session_id.is_none_or(|s| r.session_id == s));
        records.reverse();
        records.truncate(limit);
        Ok(records)
    }

    /// The stored patch of one file of a record.
    pub fn diff(&self, record_id: &str, path: &str) -> IpcResult<FileDiff> {
        let project = project_of(record_id)?;
        let text = match std::fs::read_to_string(self.patch_path(project, record_id)) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(not_found("aucun correctif enregistré pour cette modification"));
            }
            Err(e) => return Err(io_error("correctif", e)),
        };
        let file: PatchFile = serde_json::from_str(&text)
            .map_err(|e| io_error("correctif illisible", e))?;
        let found = file
            .files
            .into_iter()
            .find(|f| f.path == path)
            .ok_or_else(|| not_found("ce fichier n'a pas de correctif enregistré"))?;
        Ok(FileDiff {
            path: found.path,
            patch: found.patch,
            binary: found.binary,
            truncated: found.truncated,
        })
    }

    pub fn info(&self, project_id: &str) -> IpcResult<ChangeJournalInfo> {
        check_id(project_id)?;
        let since = self
            .read_records(project_id)?
            .iter()
            .map(|r| r.at)
            .min();
        Ok(ChangeJournalInfo {
            since,
            coverage_note: COVERAGE_NOTE.to_string(),
        })
    }

    fn journal_path(&self, project_id: &str) -> PathBuf {
        self.dir.join(format!("{project_id}.jsonl"))
    }

    fn patch_path(&self, project_id: &str, record_id: &str) -> PathBuf {
        self.dir.join(project_id).join(format!("{record_id}.json"))
    }

    /// Every readable record of a project, in the order they were written.
    /// A damaged line is skipped (and counted on stderr), never fatal.
    fn read_records(&self, project_id: &str) -> IpcResult<Vec<ChangeRecord>> {
        let text = match std::fs::read_to_string(self.journal_path(project_id)) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io_error("journal des modifications", e)),
        };
        let mut skipped = 0usize;
        let records: Vec<ChangeRecord> = text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| match serde_json::from_str(l) {
                Ok(r) => Some(r),
                Err(_) => {
                    skipped += 1;
                    None
                }
            })
            .collect();
        if skipped > 0 {
            eprintln!("bricks-desktop: {skipped} damaged line(s) skipped in the change journal of {project_id}");
        }
        Ok(records)
    }
}

fn key_of(r: &ChangeRecord) -> Key {
    (r.session_id.clone(), r.agent_id.clone(), r.tool_call_id.clone())
}

/// A new record identifier: the project id, an underscore, a random part.
pub fn new_record_id(project_id: &str) -> String {
    format!("{project_id}_{}", uuid::Uuid::new_v4().simple())
}

/// The project a record belongs to (the part before the first underscore).
fn project_of(record_id: &str) -> IpcResult<&str> {
    let (project, _) = record_id
        .split_once('_')
        .ok_or_else(|| not_found("modification inconnue"))?;
    check_id(project)?;
    check_id(record_id)?;
    Ok(project)
}

/// Identifiers become file names: letters, digits, `-`, `_` only.
fn check_id(id: &str) -> IpcResult<()> {
    let ok = !id.is_empty()
        && id.len() <= 160
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(invalid("identifiant invalide"))
    }
}

/// Appends one line, starting a new line first when the file does not end
/// with one (a write cut short by a crash must not spoil the next record).
fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
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

/// Caps a patch to [`MAX_PATCH_BYTES`] on a character boundary.
pub fn cap_patch(patch: &str) -> (String, bool) {
    if patch.len() <= MAX_PATCH_BYTES {
        return (patch.to_string(), false);
    }
    let mut end = MAX_PATCH_BYTES;
    while !patch.is_char_boundary(end) {
        end -= 1;
    }
    (patch[..end].to_string(), true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{ChangeFile, ChangeRecord};

    fn record(project: &str, session: &str, agent: Option<&str>, call: &str, at: i64) -> ChangeRecord {
        ChangeRecord {
            id: new_record_id(project),
            project_id: project.into(),
            session_id: session.into(),
            run_id: Some("run_1".into()),
            agent_id: agent.map(String::from),
            changeset_id: None,
            tool: "Write".into(),
            tool_call_id: call.into(),
            at,
            files: vec![ChangeFile {
                path: "src/main.rs".into(),
                absolute: None,
                kind: "modified".into(),
                added: 2,
                removed: 1,
                binary: false,
                has_patch: true,
            }],
        }
    }

    fn patch(path: &str) -> StoredPatch {
        StoredPatch {
            path: path.into(),
            patch: "@@ -1 +1 @@\n-a\n+b\n".into(),
            binary: false,
            truncated: false,
        }
    }

    #[test]
    fn the_same_change_is_recorded_once_even_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let journal = ChangeJournal::at(dir.path().join("changes"));
        let first = record("pAAA", "s1", None, "call_1", 10);
        let again = ChangeRecord { id: new_record_id("pAAA"), ..first.clone() };
        assert!(journal.append(NewChange { record: first, patches: vec![] }).unwrap());
        assert!(!journal.append(NewChange { record: again.clone(), patches: vec![] }).unwrap());
        // A new journal (a restarted application) reads the keys back.
        let reopened = ChangeJournal::at(dir.path().join("changes"));
        assert!(!reopened.append(NewChange { record: again, patches: vec![] }).unwrap());
        assert_eq!(reopened.list("pAAA", None, 100).unwrap().len(), 1);
    }

    #[test]
    fn a_sub_agent_tool_call_is_its_own_change() {
        let dir = tempfile::tempdir().unwrap();
        let journal = ChangeJournal::at(dir.path().to_path_buf());
        assert!(journal.append(NewChange { record: record("pAAA", "s1", None, "c", 1), patches: vec![] }).unwrap());
        assert!(journal.append(NewChange { record: record("pAAA", "s1", Some("agent_2"), "c", 2), patches: vec![] }).unwrap());
    }

    #[test]
    fn records_list_newest_first_and_filter_by_session() {
        let dir = tempfile::tempdir().unwrap();
        let journal = ChangeJournal::at(dir.path().to_path_buf());
        for (i, session) in ["s1", "s2", "s1"].iter().enumerate() {
            let r = record("pAAA", session, None, &format!("call_{i}"), i as i64);
            journal.append(NewChange { record: r, patches: vec![] }).unwrap();
        }
        let all = journal.list("pAAA", None, 100).unwrap();
        assert_eq!(all.iter().map(|r| r.tool_call_id.as_str()).collect::<Vec<_>>(), ["call_2", "call_1", "call_0"]);
        let s1 = journal.list("pAAA", Some("s1"), 100).unwrap();
        assert_eq!(s1.len(), 2);
        assert_eq!(journal.list("pAAA", None, 1).unwrap().len(), 1);
        assert!(journal.list("pBBB", None, 10).unwrap().is_empty());
    }

    #[test]
    fn a_stored_patch_is_returned_by_record_and_path_only() {
        let dir = tempfile::tempdir().unwrap();
        let journal = ChangeJournal::at(dir.path().to_path_buf());
        let r = record("pAAA", "s1", None, "call_1", 5);
        let id = r.id.clone();
        journal.append(NewChange { record: r, patches: vec![patch("src/main.rs")] }).unwrap();
        let d = journal.diff(&id, "src/main.rs").unwrap();
        assert!(d.patch.contains("+b"));
        assert_eq!(journal.diff(&id, "other.rs").unwrap_err().code, IpcErrorCode::NotFound);
        assert_eq!(journal.diff("pAAA_nope", "x").unwrap_err().code, IpcErrorCode::NotFound);
        assert_eq!(journal.diff("../etc_x", "x").unwrap_err().code, IpcErrorCode::Invalid);
    }

    #[test]
    fn info_gives_the_first_record_time_and_the_coverage_note() {
        let dir = tempfile::tempdir().unwrap();
        let journal = ChangeJournal::at(dir.path().to_path_buf());
        assert_eq!(journal.info("pAAA").unwrap().since, None);
        journal.append(NewChange { record: record("pAAA", "s1", None, "a", 300), patches: vec![] }).unwrap();
        journal.append(NewChange { record: record("pAAA", "s1", None, "b", 100), patches: vec![] }).unwrap();
        let info = journal.info("pAAA").unwrap();
        assert_eq!(info.since, Some(100));
        assert!(info.coverage_note.contains("shell"));
    }

    #[test]
    fn a_damaged_line_does_not_hide_the_others() {
        let dir = tempfile::tempdir().unwrap();
        let journal = ChangeJournal::at(dir.path().to_path_buf());
        journal.append(NewChange { record: record("pAAA", "s1", None, "a", 1), patches: vec![] }).unwrap();
        // A write cut short: no trailing newline, then a later record.
        let path = dir.path().join("pAAA.jsonl");
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("{\"id\":\"tr");
        std::fs::write(&path, text).unwrap();
        journal.append(NewChange { record: record("pAAA", "s1", None, "b", 2), patches: vec![] }).unwrap();
        let calls: Vec<String> = journal.list("pAAA", None, 10).unwrap().into_iter().map(|r| r.tool_call_id).collect();
        assert_eq!(calls, vec!["b".to_string(), "a".to_string()]);
    }

    #[test]
    fn long_patches_are_cut_on_a_character_boundary() {
        let text = "é".repeat(MAX_PATCH_BYTES);
        let (cut, truncated) = cap_patch(&text);
        assert!(truncated);
        assert!(cut.len() <= MAX_PATCH_BYTES);
        assert!(cut.chars().all(|c| c == 'é'));
        assert_eq!(cap_patch("short"), ("short".to_string(), false));
    }
}
