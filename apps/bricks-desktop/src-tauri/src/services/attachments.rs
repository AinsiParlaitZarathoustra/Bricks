//! Attachment copies owned by the desktop.
//!
//! An attachment is copied once, when it is imported (pasted, picked or
//! dropped), into a folder the desktop owns and under an opaque id:
//! * a draft, before the conversation exists:
//!   `<bricks_home>/desktop/drafts/<draft_id>/attachments/<attachment_id>-<name>`;
//! * a session, once its draft is promoted at the session's creation:
//!   `<bricks_home>/sessions/<session_id>.files/attachments/<attachment_id>-<name>`.
//!
//! The copy is independent of the original: moving, editing or deleting the
//! original afterwards changes nothing. The interface never chooses a
//! destination: it names drafts and sessions by id, and attachments by id.
//!
//! Admitted content: UTF-8 text (copies of at most [`MAX_TEXT_BYTES`]), and
//! PNG, JPEG, GIF or WebP images (at most [`MAX_IMAGE_BYTES`], the engine's
//! image limit). The type is read from the bytes, never from the name or the
//! browser. Any other format is refused with a message that names it (PDF,
//! Word, archives, other binaries): reading those needs a step of its own.
//!
//! Bounds: a draft holds at most [`MAX_DRAFT_BYTES`] of copies, a session at
//! most [`MAX_SESSION_BYTES`]. A file is read through the handle that was
//! opened, never past the bound of its type; its size is checked first.
//!
//! Symbolic links are followed once: the copy is taken from the file the link
//! names, which must be a regular file. A link to a folder, a pipe or a device,
//! and a broken link, are refused. Relative paths and paths with `..` are
//! refused. The type is checked before the file is opened, so a pipe is never
//! opened (opening it would block).
//!
//! Retention. At startup, a draft untouched for [`DRAFT_RETENTION`] is removed,
//! and so is a draft whose promotion was interrupted and cannot finish. Session
//! copies are never purged by age: they go with their session. The global
//! folder `<bricks_home>/desktop/attachments` (pasted files of earlier
//! versions) keeps its seven-day purge ([`remove_older_than`]). Its files are
//! never moved into a session: nothing proves which session, if any, they
//! belong to, so they are not migrated.

use super::paths::write_atomic;
use super::run_labels::check_session_id;
use super::{invalid, io_error, not_found};
use crate::contract::{
    AttachmentKind, AttachmentPreview, AttachmentRef, ImportedAttachment, IpcError, IpcResult,
    ResolvedAttachment,
};
use base64::Engine;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

const MIB: u64 = 1024 * 1024;

/// Largest image kept: the engine's limit for an image block.
pub const MAX_IMAGE_BYTES: u64 = 5 * MIB;
/// Largest text kept as a copy. The engine sends at most 256 KiB of a file and
/// 1 MiB per submit; a copy only needs to hold what can be asked of it.
pub const MAX_TEXT_BYTES: u64 = 2 * MIB;
/// Copies one draft may hold.
pub const MAX_DRAFT_BYTES: u64 = 50 * MIB;
/// Copies one session may hold.
pub const MAX_SESSION_BYTES: u64 = 200 * MIB;
/// A draft untouched for this long is removed at startup.
pub const DRAFT_RETENTION: Duration = Duration::from_secs(24 * 3600);
/// Pasted files of earlier versions, in the global folder.
pub const RETENTION: Duration = Duration::from_secs(7 * 24 * 3600);

/// Bytes read to recognise a format.
const SNIFF_BYTES: usize = 8 * 1024;
/// Characters of base64 accepted before decoding: the image limit, decoded.
const MAX_BASE64_CHARS: usize = (MAX_IMAGE_BYTES as usize / 3 + 1) * 4 + 4;
/// Text a preview shows at most.
const PREVIEW_TEXT_BYTES: u64 = 256 * 1024;
/// Written in a draft before its copies move into a session.
const PROMOTION_MARKER: &str = "promotion.json";
/// A file name starting with this character is a temporary file of a write
/// (`write_atomic`), never a copy.
const TEMPORARY_PREFIX: char = '.';

/// Serialises the operations on the draft and session folders of this process:
/// a draft's total is checked and its copy written in one step.
static STORE: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

// ─── Layout ──────────────────────────────────────────────────────────────────

/// `<drafts>/<draft_id>/attachments`.
pub fn draft_attachments(drafts: &Path, draft_id: &str) -> PathBuf {
    drafts.join(draft_id).join("attachments")
}

/// `<sessions>/<session_id>.files`: where the engine keeps a session's files
/// (`JsonlMemory::session_files_dir`). Deleting the session removes it.
pub fn session_files(sessions: &Path, session_id: &str) -> PathBuf {
    sessions.join(format!("{session_id}.files"))
}

/// `<session files>/attachments`.
pub fn session_attachments(files: &Path) -> PathBuf {
    files.join("attachments")
}

/// The path of the copy `<attachment_id>-<name>` in `dir`.
pub fn copy_path(dir: &Path, attachment_id: &str, name: &str) -> PathBuf {
    dir.join(format!("{attachment_id}-{name}"))
}

// ─── Ids and names ───────────────────────────────────────────────────────────

/// An opaque id: 32 lowercase hexadecimal characters.
pub fn valid_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn check_id(id: &str, what: &str) -> IpcResult<()> {
    if valid_id(id) {
        Ok(())
    } else {
        Err(invalid(format!("identifiant de {what} invalide")))
    }
}

fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// The file name as the user knows it, without any path or character that is
/// unsafe in a file name.
pub(crate) fn safe_name(name: &str) -> String {
    let base = Path::new(name)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ') {
                c
            } else {
                '_'
            }
        })
        .take(120)
        .collect();
    if cleaned.trim_matches(['.', ' ']).is_empty() {
        "fichier".to_string()
    } else {
        cleaned.trim_start_matches('.').to_string()
    }
}

// ─── Formats ─────────────────────────────────────────────────────────────────

/// The kind of an admitted file, found from its content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Image(&'static str),
    Text,
}

impl Format {
    fn cap(self) -> u64 {
        match self {
            Format::Image(_) => MAX_IMAGE_BYTES,
            Format::Text => MAX_TEXT_BYTES,
        }
    }

    fn kind(self) -> AttachmentKind {
        match self {
            Format::Image(_) => AttachmentKind::Image,
            Format::Text => AttachmentKind::Text,
        }
    }

    fn mime(self) -> &'static str {
        match self {
            Format::Image(mime) => mime,
            Format::Text => "text/plain",
        }
    }
}

fn image_mime(head: &[u8]) -> Option<&'static str> {
    if head.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("image/png")
    } else if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if head.len() >= 12 && head.starts_with(b"RIFF") && &head[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// The refusal of a format that is not admitted, named.
fn unsupported(what: &str) -> IpcError {
    invalid(format!(
        "format non pris en charge : {what} (texte UTF-8, PNG, JPEG, GIF ou WebP seulement)"
    ))
}

/// What a ZIP container is, by its extension (Office and OpenDocument files
/// are ZIP containers too).
fn zip_format(name: &str) -> &'static str {
    match Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("docx" | "dotx") => "document Word",
        Some("xlsx") => "classeur Excel",
        Some("pptx") => "présentation PowerPoint",
        _ => "archive ZIP",
    }
}

/// Recognises the format of a file from its first bytes. A format that is not
/// admitted is refused, named after what it is.
pub fn recognise(head: &[u8], name: &str) -> IpcResult<Format> {
    if let Some(mime) = image_mime(head) {
        return Ok(Format::Image(mime));
    }
    if head.starts_with(b"%PDF-") {
        return Err(unsupported("PDF"));
    }
    if head.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]) {
        return Err(unsupported(
            "document Office ancien (Word, Excel, PowerPoint)",
        ));
    }
    if head.starts_with(b"PK\x03\x04") || head.starts_with(b"PK\x05\x06") {
        return Err(unsupported(zip_format(name)));
    }
    if head.contains(&0) {
        return Err(unsupported("fichier binaire"));
    }
    match std::str::from_utf8(head) {
        Ok(_) => Ok(Format::Text),
        // The head may end inside a character: the rest decides.
        Err(e) if e.error_len().is_none() => Ok(Format::Text),
        Err(_) => Err(unsupported("texte non UTF-8")),
    }
}

/// The name an image copy keeps: the engine takes an image's type from its
/// extension, so an image whose name has another one (a PNG called `photo.txt`)
/// gets the extension of its content. A text keeps its name.
fn name_for(name: &str, format: Format) -> String {
    let Format::Image(mime) = format else {
        return name.to_string();
    };
    let (wanted, accepted): (&str, &[&str]) = match mime {
        "image/jpeg" => ("jpg", &["jpg", "jpeg"]),
        "image/gif" => ("gif", &["gif"]),
        "image/webp" => ("webp", &["webp"]),
        _ => ("png", &["png"]),
    };
    let current = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    if current.as_deref().is_some_and(|e| accepted.contains(&e)) {
        name.to_string()
    } else {
        Path::new(name)
            .with_extension(wanted)
            .to_string_lossy()
            .into_owned()
    }
}

fn check_size(format: Format, size: u64) -> IpcResult<()> {
    if size <= format.cap() {
        return Ok(());
    }
    Err(invalid(match format {
        Format::Image(_) => format!(
            "image trop volumineuse (maximum {} Mo)",
            MAX_IMAGE_BYTES / MIB
        ),
        Format::Text => format!(
            "fichier texte trop volumineux (maximum {} Mo)",
            MAX_TEXT_BYTES / MIB
        ),
    }))
}

/// Checks a whole admitted content: its size, and for a text, that it holds
/// no zero byte and is UTF-8.
fn check_whole(format: Format, content: &[u8]) -> IpcResult<()> {
    check_size(format, content.len() as u64)?;
    if format == Format::Text && (content.contains(&0) || std::str::from_utf8(content).is_err()) {
        return Err(unsupported("fichier binaire ou texte non UTF-8"));
    }
    Ok(())
}

/// The longest UTF-8 prefix of `bytes`, when what follows it is nothing or an
/// incomplete character (a read cut short). `None` when a byte is invalid.
fn utf8_prefix(bytes: &[u8]) -> Option<&str> {
    match std::str::from_utf8(bytes) {
        Ok(text) => Some(text),
        Err(e) if e.error_len().is_none() => std::str::from_utf8(&bytes[..e.valid_up_to()]).ok(),
        Err(_) => None,
    }
}

// ─── Import ──────────────────────────────────────────────────────────────────

/// Imports pasted bytes (base64; a data URL is accepted) under `name`. The
/// copy goes into `draft_id` when given (created when missing), otherwise into
/// a new draft.
pub fn import_bytes(
    drafts: &Path,
    draft_id: Option<&str>,
    name: &str,
    data_base64: &str,
) -> IpcResult<ImportedAttachment> {
    if let Some(id) = draft_id {
        check_id(id, "brouillon")?;
    }
    let data = match data_base64.strip_prefix("data:") {
        Some(rest) => rest.split_once(',').map_or("", |(_, d)| d),
        None => data_base64,
    };
    // Refused before decoding: nothing larger than the image limit is kept.
    if data.len() > MAX_BASE64_CHARS {
        return Err(invalid(format!(
            "fichier trop volumineux (maximum {} Mo pour une image)",
            MAX_IMAGE_BYTES / MIB
        )));
    }
    let content = base64::engine::general_purpose::STANDARD
        .decode(data.trim())
        .map_err(|e| invalid(format!("contenu illisible : {e}")))?;
    let name = safe_name(name);
    let format = recognise(&content[..content.len().min(SNIFF_BYTES)], &name)?;
    check_whole(format, &content)?;
    store(drafts, draft_id, &name_for(&name, format), format, &content)
}

/// Imports the file at `src`: a regular file, copied once, read through the
/// handle that was opened and never past the bound of its type. A folder is
/// not copied: it stays a reference, as the interface keeps it.
pub fn import_path(
    drafts: &Path,
    draft_id: Option<&str>,
    src: &str,
) -> IpcResult<ImportedAttachment> {
    if let Some(id) = draft_id {
        check_id(id, "brouillon")?;
    }
    let path = Path::new(src);
    if !path.is_absolute() {
        return Err(invalid("chemin absolu requis"));
    }
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(invalid("chemin non normalisé refusé"));
    }
    let name = safe_name(
        &path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    );
    let link = std::fs::symlink_metadata(path).map_err(|e| missing_or_io(e, src))?;
    let target = if link.file_type().is_symlink() {
        std::fs::canonicalize(path).map_err(|_| invalid("lien symbolique cassé"))?
    } else {
        path.to_path_buf()
    };
    // The type is checked before the file is opened: a pipe would block the open.
    let meta = std::fs::metadata(&target).map_err(|e| missing_or_io(e, src))?;
    refuse_unless_regular(&meta)?;
    let mut file = File::open(&target).map_err(|e| io_error("pièce jointe", e))?;
    let opened = file.metadata().map_err(|e| io_error("pièce jointe", e))?;
    refuse_unless_regular(&opened)?;
    let size = opened.len();

    let mut head = Vec::new();
    (&mut file)
        .take(SNIFF_BYTES as u64)
        .read_to_end(&mut head)
        .map_err(|e| io_error("pièce jointe", e))?;
    let format = recognise(&head, &name)?;
    // An image over its limit is refused without reading it.
    check_size(format, size)?;

    file.seek(SeekFrom::Start(0))
        .map_err(|e| io_error("pièce jointe", e))?;
    let mut content = Vec::with_capacity(size as usize);
    file.take(format.cap() + 1)
        .read_to_end(&mut content)
        .map_err(|e| io_error("pièce jointe", e))?;
    // A file that grew while it was read is refused by its new size.
    check_whole(format, &content)?;
    store(drafts, draft_id, &name_for(&name, format), format, &content)
}

fn refuse_unless_regular(meta: &std::fs::Metadata) -> IpcResult<()> {
    if meta.is_dir() {
        return Err(invalid(
            "un dossier n'est pas copié : il reste une référence",
        ));
    }
    if !meta.is_file() {
        return Err(invalid("seuls les fichiers réguliers sont copiés"));
    }
    Ok(())
}

fn missing_or_io(e: std::io::Error, src: &str) -> IpcError {
    if e.kind() == std::io::ErrorKind::NotFound {
        not_found(format!("élément introuvable : {src}"))
    } else {
        io_error("pièce jointe", e)
    }
}

/// Writes the copy into the draft and returns it. A new draft id is made when
/// none is given.
fn store(
    drafts: &Path,
    draft_id: Option<&str>,
    name: &str,
    format: Format,
    content: &[u8],
) -> IpcResult<ImportedAttachment> {
    let draft = match draft_id {
        Some(id) => {
            check_id(id, "brouillon")?;
            id.to_string()
        }
        None => new_id(),
    };
    let _guard = STORE.lock();
    let dir = draft_attachments(drafts, &draft);
    if folder_bytes(&dir) + content.len() as u64 > MAX_DRAFT_BYTES {
        return Err(invalid(format!(
            "brouillon plein : ses pièces jointes ne peuvent pas dépasser {} Mo",
            MAX_DRAFT_BYTES / MIB
        )));
    }
    let id = new_id();
    write_atomic(&copy_path(&dir, &id, name), content).map_err(|e| io_error("pièce jointe", e))?;
    Ok(ImportedAttachment {
        draft_id: draft,
        attachment: AttachmentRef {
            id,
            name: name.to_string(),
            kind: format.kind(),
            mime: format.mime().to_string(),
            size: content.len() as u64,
        },
    })
}

// ─── Discard, promotion, recovery ────────────────────────────────────────────

/// Removes the copy `attachment_id` of a draft. Idempotent.
pub fn discard_draft_attachment(
    drafts: &Path,
    draft_id: &str,
    attachment_id: &str,
) -> IpcResult<()> {
    check_id(draft_id, "brouillon")?;
    check_id(attachment_id, "pièce jointe")?;
    let _guard = STORE.lock();
    remove_copy(&draft_attachments(drafts, draft_id), attachment_id)
}

/// Removes the copy `attachment_id` kept in `dir`. Idempotent.
pub fn remove_copy(dir: &Path, attachment_id: &str) -> IpcResult<()> {
    for (id, path) in copies(dir) {
        if id == attachment_id {
            std::fs::remove_file(&path).map_err(|e| io_error("pièce jointe", e))?;
        }
    }
    Ok(())
}

/// Removes a draft and every copy it holds. Idempotent.
pub fn discard_draft(drafts: &Path, draft_id: &str) -> IpcResult<()> {
    check_id(draft_id, "brouillon")?;
    let _guard = STORE.lock();
    remove_draft_dir(drafts, draft_id)
}

fn remove_draft_dir(drafts: &Path, draft_id: &str) -> IpcResult<()> {
    match std::fs::remove_dir_all(drafts.join(draft_id)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_error("brouillon", e)),
    }
}

/// Moves the copies of a draft into a session. The session must exist (its
/// folder is there from its first open). The draft's marker is written first,
/// so a promotion cut short by a crash is finished at the next startup (see
/// [`recover_promotions`]). Returns the copies as the session now holds them;
/// nothing when the draft holds none (already promoted).
pub fn promote(
    drafts: &Path,
    sessions: &Path,
    draft_id: &str,
    session_id: &str,
) -> IpcResult<Vec<AttachmentRef>> {
    check_id(draft_id, "brouillon")?;
    check_session_id(session_id)?;
    let _guard = STORE.lock();
    let draft = drafts.join(draft_id);
    let sources = copies(&draft.join("attachments"));
    if sources.is_empty() {
        remove_draft_dir(drafts, draft_id)?;
        return Ok(Vec::new());
    }
    let files = session_files(sessions, session_id);
    if !files.is_dir() {
        return Err(not_found("conversation inconnue"));
    }
    let incoming: u64 = sources.iter().map(|(_, p)| file_len(p)).sum();
    if folder_bytes(&session_attachments(&files)) + incoming > MAX_SESSION_BYTES {
        return Err(invalid(format!(
            "conversation pleine : ses pièces jointes ne peuvent pas dépasser {} Mo",
            MAX_SESSION_BYTES / MIB
        )));
    }
    // A retry after a promotion that failed midway: the copies it already
    // moved are part of this promotion too, and are returned again.
    let previous = read_marker(&draft)
        .filter(|(session, _)| session == session_id)
        .map(|(_, copies)| copies)
        .unwrap_or_default();
    let mut marked: Vec<String> = sources.iter().map(|(id, _)| id.clone()).collect();
    for id in &previous {
        if !marked.contains(id) {
            marked.push(id.clone());
        }
    }
    write_marker(&draft, session_id, &marked)?;
    let mut refs = move_into(&draft, &files)?;
    let target = session_attachments(&files);
    for id in previous {
        if refs.iter().any(|r| r.id == id) {
            continue;
        }
        if let Some((_, path)) = copies(&target).into_iter().find(|(i, _)| *i == id)
            && let Some(attachment) = attachment_of_copy(&path)
        {
            refs.push(attachment);
        }
    }
    remove_dir(&draft)?;
    Ok(refs)
}

/// Writes the marker of a promotion: its session and the copies it moves.
fn write_marker(draft: &Path, session_id: &str, copies: &[String]) -> IpcResult<()> {
    let marker = serde_json::json!({ "session_id": session_id, "copies": copies });
    write_atomic(&draft.join(PROMOTION_MARKER), marker.to_string().as_bytes())
        .map_err(|e| io_error("promotion", e))
}

/// The session and the copies recorded by a draft's marker, if it has one.
fn read_marker(draft: &Path) -> Option<(String, Vec<String>)> {
    let text = std::fs::read_to_string(draft.join(PROMOTION_MARKER)).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let session = value.get("session_id")?.as_str()?.to_string();
    let copies = value
        .get("copies")
        .and_then(|c| c.as_array())
        .map(|list| {
            list.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    Some((session, copies))
}

/// Moves every copy of the draft at `draft` into the session folder `files`,
/// one rename each (the same volume, so each move is atomic). Returns the
/// copies as the session holds them.
fn move_into(draft: &Path, files: &Path) -> IpcResult<Vec<AttachmentRef>> {
    let target = session_attachments(files);
    std::fs::create_dir_all(&target)
        .map_err(|e| io_error("pièces jointes de la conversation", e))?;
    let mut refs = Vec::new();
    for (_, source) in copies(&draft.join("attachments")) {
        let Some(file_name) = source.file_name() else {
            continue;
        };
        let dest = target.join(file_name);
        std::fs::rename(&source, &dest).map_err(|e| io_error("promotion d'une pièce jointe", e))?;
        if let Some(attachment) = attachment_of_copy(&dest) {
            refs.push(attachment);
        }
    }
    Ok(refs)
}

fn remove_dir(dir: &Path) -> IpcResult<()> {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_error("brouillon", e)),
    }
}

/// At startup: finishes the promotions a crash interrupted, when their session
/// still exists, and removes the drafts whose session does not. Drafts without
/// a marker are left to [`sweep_drafts`].
pub fn recover_promotions(drafts: &Path, sessions: &Path) {
    let _guard = STORE.lock();
    for draft in subfolders(drafts) {
        let Some((session, _)) = read_marker(&draft) else {
            continue;
        };
        let files = Some(session)
            .filter(|id| check_session_id(id).is_ok())
            .map(|id| session_files(sessions, &id))
            .filter(|files| files.is_dir());
        let result = match files {
            Some(files) => move_into(&draft, &files).and_then(|_| remove_dir(&draft)),
            None => remove_dir(&draft),
        };
        if let Err(e) = result {
            eprintln!(
                "bricks-desktop: promotion interrompue de {} : {}",
                draft.display(),
                e.message
            );
        }
    }
}

/// Removes the drafts untouched since `now - retention`: the most recent
/// modification of a draft counts, down to its copies. Returns how many went.
pub fn sweep_drafts(drafts: &Path, now: SystemTime, retention: Duration) -> usize {
    let Some(cutoff) = now.checked_sub(retention) else {
        return 0;
    };
    let _guard = STORE.lock();
    let mut removed = 0;
    for draft in subfolders(drafts) {
        let stale = newest_modified(&draft, 3).is_some_and(|t| t < cutoff);
        if stale && std::fs::remove_dir_all(&draft).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// The most recent modification time of `path` and of what is under it, down
/// `depth` levels. `None` when `path` cannot be read.
fn newest_modified(path: &Path, depth: u32) -> Option<SystemTime> {
    let own = std::fs::metadata(path).ok()?.modified().ok()?;
    if depth == 0 {
        return Some(own);
    }
    let mut newest = own;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            if let Some(t) = newest_modified(&entry.path(), depth - 1)
                && t > newest
            {
                newest = t;
            }
        }
    }
    Some(newest)
}

/// Deletes the files of `dir` last modified before `older_than`. Returns how
/// many were removed; failures are skipped (a file in use stays). Only the
/// global folder of earlier versions is purged this way; subfolders are never
/// touched.
pub fn remove_older_than(dir: &Path, older_than: Duration) -> usize {
    let Some(cutoff) = SystemTime::now().checked_sub(older_than) else {
        return 0;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .map(|t| t < cutoff)
            .unwrap_or(false);
        if old && entry.path().is_file() && std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

// ─── Reading copies ──────────────────────────────────────────────────────────

/// The copies directly in `dir`, by attachment id. Temporary files of a write
/// (dot-named) and anything else are left out.
fn copies(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let id = {
                let file = path.file_name()?.to_str()?;
                if file.starts_with(TEMPORARY_PREFIX) {
                    return None;
                }
                file.split_once('-')?.0.to_string()
            };
            (valid_id(&id) && path.is_file()).then_some((id, path))
        })
        .collect();
    found.sort();
    found
}

/// The directories directly in `dir`.
fn subfolders(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect()
}

/// The bytes of the regular files directly in `dir` (0 when there is none).
fn folder_bytes(dir: &Path) -> u64 {
    copies(dir).iter().map(|(_, p)| file_len(p)).sum()
}

fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// The reference of a copy file `<id>-<name>`, from its name and content.
/// `None` when the file is not a copy.
pub fn attachment_of_copy(path: &Path) -> Option<AttachmentRef> {
    let file = path.file_name()?.to_str()?;
    let (id, name) = file.split_once('-')?;
    if !valid_id(id) || name.is_empty() {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let mut head = Vec::new();
    File::open(path)
        .ok()?
        .take(16)
        .read_to_end(&mut head)
        .ok()?;
    let format = match image_mime(&head) {
        Some(mime) => Format::Image(mime),
        None => Format::Text,
    };
    Some(AttachmentRef {
        id: id.to_string(),
        name: name.to_string(),
        kind: format.kind(),
        mime: format.mime().to_string(),
        size: meta.len(),
    })
}

/// The absolute paths of the copies `ids` kept in `dir`, in the order given.
/// Refused when one of them is not there.
pub fn resolve(dir: &Path, ids: &[String]) -> IpcResult<Vec<ResolvedAttachment>> {
    let found = copies(dir);
    ids.iter()
        .map(|id| {
            check_id(id, "pièce jointe")?;
            let (_, path) = found
                .iter()
                .find(|(i, _)| i == id)
                .ok_or_else(|| not_found("pièce jointe introuvable"))?;
            let attachment =
                attachment_of_copy(path).ok_or_else(|| not_found("pièce jointe introuvable"))?;
            Ok(ResolvedAttachment {
                id: id.clone(),
                kind: attachment.kind,
                path: path.display().to_string(),
            })
        })
        .collect()
}

/// The copy `attachment_id` kept in `dir`, as the interface shows it. Reads are
/// bounded: an image is read whole (it is at most its limit, and a copy over it
/// is refused), a text up to its first 256 KiB.
pub fn preview(dir: &Path, attachment_id: &str) -> IpcResult<AttachmentPreview> {
    check_id(attachment_id, "pièce jointe")?;
    let path = copies(dir)
        .into_iter()
        .find(|(id, _)| id == attachment_id)
        .map(|(_, p)| p)
        .ok_or_else(|| not_found("pièce jointe introuvable"))?;
    let attachment =
        attachment_of_copy(&path).ok_or_else(|| not_found("pièce jointe introuvable"))?;
    let read = |limit: u64| -> IpcResult<Vec<u8>> {
        let file = File::open(&path).map_err(|e| io_error("pièce jointe", e))?;
        let mut bytes = Vec::new();
        file.take(limit)
            .read_to_end(&mut bytes)
            .map_err(|e| io_error("pièce jointe", e))?;
        Ok(bytes)
    };
    match attachment.kind {
        AttachmentKind::Image => {
            let bytes = read(MAX_IMAGE_BYTES + 1)?;
            if bytes.len() as u64 > MAX_IMAGE_BYTES {
                return Err(invalid(format!(
                    "image trop volumineuse (maximum {} Mo)",
                    MAX_IMAGE_BYTES / MIB
                )));
            }
            let data_url = format!(
                "data:{};base64,{}",
                attachment.mime,
                base64::engine::general_purpose::STANDARD.encode(&bytes)
            );
            Ok(AttachmentPreview {
                attachment,
                data_url: Some(data_url),
                text: None,
                truncated: false,
            })
        }
        AttachmentKind::Text => {
            let bytes = read(PREVIEW_TEXT_BYTES + 1)?;
            let truncated = bytes.len() as u64 > PREVIEW_TEXT_BYTES;
            let kept = bytes.len().min(PREVIEW_TEXT_BYTES as usize);
            let text = utf8_prefix(&bytes[..kept])
                .ok_or_else(|| unsupported("texte non UTF-8"))?
                .to_string();
            Ok(AttachmentPreview {
                attachment,
                data_url: None,
                text: Some(text),
                truncated,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1×1 PNG (67 bytes).
    const PNG_1X1: [u8; 67] = [
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    /// Writes a sparse file: `head` at the start, `size` bytes long (what
    /// follows the head is zeros that take no disk space).
    fn sparse(path: &Path, head: &[u8], size: u64) {
        use std::io::Write;
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut file = File::create(path).unwrap();
        file.write_all(head).unwrap();
        file.set_len(size).unwrap();
    }

    /// Sets the modification time of a file or of a folder.
    fn age(path: &Path, when: SystemTime) {
        File::open(path).unwrap().set_modified(when).unwrap();
    }

    fn draft_copies(drafts: &Path, draft: &str) -> Vec<String> {
        copies(&draft_attachments(drafts, draft))
            .into_iter()
            .map(|(_, p)| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn a_pasted_image_and_a_text_are_copied_under_opaque_ids() {
        let root = tempfile::tempdir().unwrap();
        let drafts = root.path().join("drafts");
        let img = import_bytes(&drafts, None, "capture.png", &b64(&PNG_1X1)).unwrap();
        assert!(valid_id(&img.attachment.id));
        assert!(
            valid_id(&img.draft_id),
            "a draft id is made when none is given"
        );
        assert_eq!(img.attachment.kind, AttachmentKind::Image);
        assert_eq!(img.attachment.mime, "image/png");
        assert_eq!(img.attachment.size, 67);
        assert_eq!(img.attachment.name, "capture.png");

        // The same draft takes the next copy.
        let txt = import_bytes(&drafts, Some(&img.draft_id), "notes.md", &b64(b"bonjour")).unwrap();
        assert_eq!(txt.draft_id, img.draft_id);
        assert_eq!(txt.attachment.kind, AttachmentKind::Text);
        assert_eq!(txt.attachment.mime, "text/plain");
        let names = draft_copies(&drafts, &img.draft_id);
        assert_eq!(names.len(), 2);
        assert!(names.contains(&format!("{}-capture.png", img.attachment.id)));
        assert!(names.contains(&format!("{}-notes.md", txt.attachment.id)));
    }

    #[test]
    fn a_data_url_is_accepted_and_the_content_decides_the_type() {
        let root = tempfile::tempdir().unwrap();
        let drafts = root.path().join("drafts");
        // Declared as a text by its name, but the bytes are a PNG: the copy
        // takes the extension of its content, which the engine reads.
        let url = format!("data:image/jpeg;base64,{}", b64(&PNG_1X1));
        let got = import_bytes(&drafts, None, "photo.txt", &url).unwrap();
        assert_eq!(got.attachment.kind, AttachmentKind::Image);
        assert_eq!(got.attachment.mime, "image/png");
        assert_eq!(got.attachment.name, "photo.png");
        let jpg = import_bytes(&drafts, None, "Scan.JPEG", &b64(&PNG_1X1)).unwrap();
        assert_eq!(
            jpg.attachment.name, "Scan.png",
            "the extension follows the content"
        );
        let kept = import_bytes(&drafts, None, "ok.PNG", &b64(&PNG_1X1)).unwrap();
        assert_eq!(
            kept.attachment.name, "ok.PNG",
            "a matching extension is kept"
        );
    }

    #[test]
    fn a_path_is_copied_and_the_copy_outlives_the_original() {
        let root = tempfile::tempdir().unwrap();
        let drafts = root.path().join("drafts");
        let original = root.path().join("src").join("rapport.txt");
        std::fs::create_dir_all(original.parent().unwrap()).unwrap();
        std::fs::write(&original, "version un").unwrap();
        let got = import_path(&drafts, None, original.to_str().unwrap()).unwrap();
        let copy = copy_path(
            &draft_attachments(&drafts, &got.draft_id),
            &got.attachment.id,
            &got.attachment.name,
        );
        // Edited, then deleted: the copy keeps what was imported.
        std::fs::write(&original, "version deux").unwrap();
        assert_eq!(std::fs::read_to_string(&copy).unwrap(), "version un");
        std::fs::remove_file(&original).unwrap();
        assert_eq!(std::fs::read_to_string(&copy).unwrap(), "version un");
    }

    #[test]
    fn an_image_over_its_limit_is_refused_without_being_read() {
        let root = tempfile::tempdir().unwrap();
        let drafts = root.path().join("drafts");
        let big = root.path().join("big.png");
        // A PNG header, and a file 6 MiB long (sparse: nothing is written after it).
        sparse(&big, &PNG_1X1[..8], 6 * MIB);
        let err = import_path(&drafts, None, big.to_str().unwrap()).unwrap_err();
        assert_eq!(err.code, crate::contract::IpcErrorCode::Invalid);
        assert!(
            err.message.contains("image trop volumineuse"),
            "{}",
            err.message
        );
        assert!(!drafts.exists(), "nothing is written");
        // The same bound applies to pasted bytes, before they are decoded.
        let pasted = b64(&vec![0u8; 6 * MIB as usize]);
        assert!(import_bytes(&drafts, None, "x.png", &pasted).is_err());
    }

    #[test]
    fn a_text_over_its_copy_cap_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let drafts = root.path().join("drafts");
        let long = root.path().join("long.txt");
        sparse(&long, &vec![b'a'; 8192], MAX_TEXT_BYTES + 1);
        let err = import_path(&drafts, None, long.to_str().unwrap()).unwrap_err();
        assert!(
            err.message.contains("fichier texte trop volumineux"),
            "{}",
            err.message
        );
    }

    #[test]
    fn pdf_word_archives_and_binaries_are_refused_by_name() {
        let root = tempfile::tempdir().unwrap();
        let drafts = root.path().join("drafts");
        let pdf = import_bytes(&drafts, None, "cv.pdf", &b64(b"%PDF-1.7\n1 0 obj")).unwrap_err();
        assert!(
            pdf.message.starts_with("format non pris en charge : PDF"),
            "{}",
            pdf.message
        );
        let word = import_bytes(&drafts, None, "lettre.docx", &b64(b"PK\x03\x04rest")).unwrap_err();
        assert!(word.message.contains("document Word"), "{}", word.message);
        let zip = import_bytes(&drafts, None, "lot.zip", &b64(b"PK\x03\x04rest")).unwrap_err();
        assert!(zip.message.contains("archive ZIP"), "{}", zip.message);
        let bin = import_bytes(&drafts, None, "a.dat", &b64(&[1, 2, 0, 3])).unwrap_err();
        assert!(bin.message.contains("fichier binaire"), "{}", bin.message);
        let latin =
            import_bytes(&drafts, None, "a.txt", &b64(&[0x63, 0xE9, 0x20, 0xFF])).unwrap_err();
        assert!(
            latin.message.contains("texte non UTF-8"),
            "{}",
            latin.message
        );
        assert!(!drafts.exists(), "nothing is kept");
    }

    #[test]
    fn a_text_cut_inside_a_character_by_the_head_is_still_text() {
        let root = tempfile::tempdir().unwrap();
        let drafts = root.path().join("drafts");
        // 9000 bytes of three-byte characters: the 8 KiB head ends mid-character.
        let text = "€".repeat(3000);
        let got = import_bytes(&drafts, None, "long.txt", &b64(text.as_bytes())).unwrap();
        assert_eq!(got.attachment.kind, AttachmentKind::Text);
    }

    #[test]
    fn names_are_made_safe_and_unique() {
        let root = tempfile::tempdir().unwrap();
        let drafts = root.path().join("drafts");
        let a = import_bytes(&drafts, None, "../dossier/a:b.txt", &b64(b"1")).unwrap();
        let b = import_bytes(&drafts, Some(&a.draft_id), "a:b.txt", &b64(b"2")).unwrap();
        assert_eq!(a.attachment.name, "a_b.txt");
        assert_ne!(a.attachment.id, b.attachment.id);
        assert_eq!(draft_copies(&drafts, &a.draft_id).len(), 2);
        assert_eq!(safe_name(""), "fichier");
        assert_eq!(safe_name(".."), "fichier");
    }

    #[test]
    fn links_are_followed_once_and_unsafe_ones_are_refused() {
        let root = tempfile::tempdir().unwrap();
        let drafts = root.path().join("drafts");
        let target = root.path().join("cible.txt");
        std::fs::write(&target, "contenu").unwrap();
        let link = root.path().join("lien.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let got = import_path(&drafts, None, link.to_str().unwrap()).unwrap();
        assert_eq!(got.attachment.name, "lien.txt", "the name is the link's");
        assert_eq!(got.attachment.size, 7);

        let folder = root.path().join("dossier");
        std::fs::create_dir_all(&folder).unwrap();
        let to_folder = root.path().join("vers-dossier");
        std::os::unix::fs::symlink(&folder, &to_folder).unwrap();
        let err = import_path(&drafts, None, to_folder.to_str().unwrap()).unwrap_err();
        assert!(err.message.contains("dossier"), "{}", err.message);

        let dangling = root.path().join("casse.txt");
        std::os::unix::fs::symlink(root.path().join("absent"), &dangling).unwrap();
        let err = import_path(&drafts, None, dangling.to_str().unwrap()).unwrap_err();
        assert!(
            err.message.contains("lien symbolique cassé"),
            "{}",
            err.message
        );
    }

    #[test]
    fn a_pipe_is_refused_without_being_opened() {
        let root = tempfile::tempdir().unwrap();
        let drafts = root.path().join("drafts");
        let pipe = root.path().join("tube");
        let made = std::process::Command::new("mkfifo")
            .arg(&pipe)
            .status()
            .unwrap();
        assert!(made.success());
        // Opening a pipe without a writer would block: the refusal must come first.
        let err = import_path(&drafts, None, pipe.to_str().unwrap()).unwrap_err();
        assert!(err.message.contains("réguliers"), "{}", err.message);
    }

    #[test]
    fn relative_paths_and_traversal_are_refused() {
        let root = tempfile::tempdir().unwrap();
        let drafts = root.path().join("drafts");
        let err = import_path(&drafts, None, "relatif.txt").unwrap_err();
        assert!(err.message.contains("absolu"), "{}", err.message);
        let inside = root.path().join("a.txt");
        std::fs::write(&inside, "x").unwrap();
        let sneaky = format!("{}/../{}", root.path().join("drafts").display(), "a.txt");
        let err = import_path(&drafts, None, &sneaky).unwrap_err();
        assert!(err.message.contains("normalisé"), "{}", err.message);
    }

    #[test]
    fn a_draft_holds_at_most_its_bound() {
        let root = tempfile::tempdir().unwrap();
        let drafts = root.path().join("drafts");
        let draft = new_id();
        let dir = draft_attachments(&drafts, &draft);
        // 25 copies of 2 MiB fill the 50 MiB bound (sparse files).
        for i in 0..25 {
            sparse(
                &copy_path(&dir, &new_id(), &format!("f{i}.txt")),
                b"x",
                MAX_TEXT_BYTES,
            );
        }
        let err = import_bytes(&drafts, Some(&draft), "more.txt", &b64(b"encore")).unwrap_err();
        assert!(err.message.contains("brouillon plein"), "{}", err.message);
        // Another draft is not affected.
        assert!(import_bytes(&drafts, None, "more.txt", &b64(b"encore")).is_ok());
    }

    #[test]
    fn discarding_frees_the_copy_and_is_idempotent() {
        let root = tempfile::tempdir().unwrap();
        let drafts = root.path().join("drafts");
        let a = import_bytes(&drafts, None, "a.txt", &b64(b"1")).unwrap();
        let b = import_bytes(&drafts, Some(&a.draft_id), "b.txt", &b64(b"2")).unwrap();
        discard_draft_attachment(&drafts, &a.draft_id, &a.attachment.id).unwrap();
        discard_draft_attachment(&drafts, &a.draft_id, &a.attachment.id).unwrap();
        assert_eq!(
            draft_copies(&drafts, &a.draft_id),
            vec![format!("{}-b.txt", b.attachment.id)]
        );
        discard_draft(&drafts, &a.draft_id).unwrap();
        discard_draft(&drafts, &a.draft_id).unwrap();
        assert!(!drafts.join(&a.draft_id).exists());
    }

    #[test]
    fn a_promotion_moves_the_copies_into_the_session_and_removes_the_draft() {
        let root = tempfile::tempdir().unwrap();
        let drafts = root.path().join("drafts");
        let sessions = root.path().join("sessions");
        let session = "20260101-120000-abcdef";
        std::fs::create_dir_all(session_files(&sessions, session)).unwrap();
        let a = import_bytes(&drafts, None, "a.png", &b64(&PNG_1X1)).unwrap();
        let b = import_bytes(&drafts, Some(&a.draft_id), "b.txt", &b64(b"texte")).unwrap();

        let refs = promote(&drafts, &sessions, &a.draft_id, session).unwrap();
        assert_eq!(refs.len(), 2);
        assert!(
            refs.iter()
                .any(|r| r.id == a.attachment.id && r.kind == AttachmentKind::Image)
        );
        assert!(refs.iter().any(|r| r.id == b.attachment.id && r.size == 5));
        assert!(!drafts.join(&a.draft_id).exists(), "the draft is gone");
        let dir = session_attachments(&session_files(&sessions, session));
        assert_eq!(copies(&dir).len(), 2);

        // Promoting again finds nothing to move.
        assert!(
            promote(&drafts, &sessions, &a.draft_id, session)
                .unwrap()
                .is_empty()
        );

        // The session's copies resolve to paths in its folder.
        let ids = vec![b.attachment.id.clone(), a.attachment.id.clone()];
        let resolved = resolve(&dir, &ids).unwrap();
        assert_eq!(resolved[0].id, b.attachment.id);
        assert_eq!(resolved[0].kind, AttachmentKind::Text);
        assert!(resolved[1].path.starts_with(&dir.display().to_string()));
        let missing = resolve(&dir, &[new_id()]).unwrap_err();
        assert_eq!(missing.code, crate::contract::IpcErrorCode::NotFound);
    }

    #[test]
    fn a_promotion_without_its_session_keeps_the_draft() {
        let root = tempfile::tempdir().unwrap();
        let drafts = root.path().join("drafts");
        let sessions = root.path().join("sessions");
        let a = import_bytes(&drafts, None, "a.txt", &b64(b"1")).unwrap();
        let err = promote(&drafts, &sessions, &a.draft_id, "20260101-120000-abcdef").unwrap_err();
        assert_eq!(err.code, crate::contract::IpcErrorCode::NotFound);
        assert_eq!(
            draft_copies(&drafts, &a.draft_id).len(),
            1,
            "the copy is kept"
        );
    }

    #[test]
    fn an_interrupted_promotion_is_finished_at_startup() {
        let root = tempfile::tempdir().unwrap();
        let drafts = root.path().join("drafts");
        let sessions = root.path().join("sessions");
        let session = "20260101-120000-abcdef";
        std::fs::create_dir_all(session_files(&sessions, session)).unwrap();
        let a = import_bytes(&drafts, None, "a.txt", &b64(b"1")).unwrap();
        let b = import_bytes(&drafts, Some(&a.draft_id), "b.txt", &b64(b"2")).unwrap();
        // The crash: the marker is written and only one copy has moved.
        let draft = drafts.join(&a.draft_id);
        write_atomic(
            &draft.join(PROMOTION_MARKER),
            format!("{{\"session_id\":\"{session}\"}}").as_bytes(),
        )
        .unwrap();
        let moved = copy_path(
            &session_attachments(&session_files(&sessions, session)),
            &a.attachment.id,
            "a.txt",
        );
        std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
        std::fs::rename(
            copy_path(
                &draft_attachments(&drafts, &a.draft_id),
                &a.attachment.id,
                "a.txt",
            ),
            &moved,
        )
        .unwrap();

        recover_promotions(&drafts, &sessions);
        assert!(!draft.exists());
        let dir = session_attachments(&session_files(&sessions, session));
        let ids: Vec<String> = copies(&dir).into_iter().map(|(id, _)| id).collect();
        assert!(ids.contains(&a.attachment.id) && ids.contains(&b.attachment.id));
    }

    #[test]
    fn a_retried_promotion_returns_the_copies_an_earlier_attempt_moved() {
        let root = tempfile::tempdir().unwrap();
        let drafts = root.path().join("drafts");
        let sessions = root.path().join("sessions");
        let session = "20260101-120000-abcdef";
        std::fs::create_dir_all(session_files(&sessions, session)).unwrap();
        let a = import_bytes(&drafts, None, "a.txt", &b64(b"1")).unwrap();
        let b = import_bytes(&drafts, Some(&a.draft_id), "b.txt", &b64(b"2")).unwrap();
        // The first attempt wrote its marker, then moved only the first copy.
        let draft = drafts.join(&a.draft_id);
        write_marker(
            &draft,
            session,
            &[a.attachment.id.clone(), b.attachment.id.clone()],
        )
        .unwrap();
        let target = session_attachments(&session_files(&sessions, session));
        std::fs::create_dir_all(&target).unwrap();
        std::fs::rename(
            copy_path(
                &draft_attachments(&drafts, &a.draft_id),
                &a.attachment.id,
                "a.txt",
            ),
            copy_path(&target, &a.attachment.id, "a.txt"),
        )
        .unwrap();

        let refs = promote(&drafts, &sessions, &a.draft_id, session).unwrap();
        let mut ids: Vec<String> = refs.iter().map(|r| r.id.clone()).collect();
        ids.sort();
        let mut want = vec![a.attachment.id.clone(), b.attachment.id.clone()];
        want.sort();
        assert_eq!(ids, want, "both copies are returned to the retry");
        assert!(!draft.exists());
    }

    #[test]
    fn an_interrupted_promotion_of_a_missing_session_is_dropped() {
        let root = tempfile::tempdir().unwrap();
        let drafts = root.path().join("drafts");
        let sessions = root.path().join("sessions");
        let a = import_bytes(&drafts, None, "a.txt", &b64(b"1")).unwrap();
        write_atomic(
            &drafts.join(&a.draft_id).join(PROMOTION_MARKER),
            b"{\"session_id\":\"20260101-120000-abcdef\"}",
        )
        .unwrap();
        recover_promotions(&drafts, &sessions);
        assert!(!drafts.join(&a.draft_id).exists());
    }

    #[test]
    fn the_sweep_removes_stale_drafts_and_keeps_recent_ones() {
        let root = tempfile::tempdir().unwrap();
        let drafts = root.path().join("drafts");
        let old = import_bytes(&drafts, None, "old.txt", &b64(b"1")).unwrap();
        let recent = import_bytes(&drafts, None, "recent.txt", &b64(b"2")).unwrap();
        let stale = copy_path(
            &draft_attachments(&drafts, &old.draft_id),
            &old.attachment.id,
            "old.txt",
        );
        let long_ago = SystemTime::now() - Duration::from_secs(3 * 24 * 3600);
        // The draft's folders and its copy all count: each one is aged.
        age(&stale, long_ago);
        age(&draft_attachments(&drafts, &old.draft_id), long_ago);
        age(&drafts.join(&old.draft_id), long_ago);
        assert_eq!(sweep_drafts(&drafts, SystemTime::now(), DRAFT_RETENTION), 1);
        assert!(!drafts.join(&old.draft_id).exists());
        assert!(drafts.join(&recent.draft_id).exists());
    }

    #[test]
    fn the_sweep_never_touches_session_copies() {
        let root = tempfile::tempdir().unwrap();
        let drafts = root.path().join("drafts");
        let sessions = root.path().join("sessions");
        let session = "20260101-120000-abcdef";
        let kept = copy_path(
            &session_attachments(&session_files(&sessions, session)),
            &new_id(),
            "k.txt",
        );
        sparse(&kept, b"k", 1);
        let later = SystemTime::now() + Duration::from_secs(60 * 24 * 3600);
        sweep_drafts(&drafts, later, DRAFT_RETENTION);
        recover_promotions(&drafts, &sessions);
        assert!(kept.exists());
    }

    #[test]
    fn the_session_folder_is_the_engine_layout() {
        use bricks_memory::{JsonlMemory, Memory};
        let root = tempfile::tempdir().unwrap();
        let session = "20260101-120000-abcdef";
        let ours = session_files(root.path(), session);
        let engine = JsonlMemory::new(root.path())
            .session_files_dir(session)
            .unwrap();
        assert_eq!(ours, engine);
    }

    #[test]
    fn the_global_folder_keeps_its_purge_of_old_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("recent"), "r").unwrap();
        assert_eq!(
            remove_older_than(dir.path(), Duration::from_secs(3600)),
            0,
            "written just now: kept"
        );
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(remove_older_than(dir.path(), Duration::from_millis(5)), 1);
        assert!(!dir.path().join("recent").exists());
    }

    #[test]
    fn a_preview_is_an_image_as_a_data_url_and_a_long_text_is_cut() {
        let root = tempfile::tempdir().unwrap();
        let drafts = root.path().join("drafts");
        let img = import_bytes(&drafts, None, "p.png", &b64(&PNG_1X1)).unwrap();
        let dir = draft_attachments(&drafts, &img.draft_id);
        let shown = preview(&dir, &img.attachment.id).unwrap();
        assert!(
            shown
                .data_url
                .unwrap()
                .starts_with("data:image/png;base64,")
        );
        assert!(!shown.truncated);

        let long = import_bytes(
            &drafts,
            Some(&img.draft_id),
            "long.txt",
            &b64("a".repeat(300 * 1024).as_bytes()),
        )
        .unwrap();
        let text = preview(&dir, &long.attachment.id).unwrap();
        assert!(text.truncated);
        assert_eq!(text.text.unwrap().len(), 256 * 1024);

        // A copy over the image limit is not read at all.
        let big = copy_path(&dir, &new_id(), "huge.png");
        sparse(&big, &PNG_1X1[..8], MAX_IMAGE_BYTES + 1);
        let id = big
            .file_name()
            .unwrap()
            .to_string_lossy()
            .split('-')
            .next()
            .unwrap()
            .to_string();
        assert!(
            preview(&dir, &id)
                .unwrap_err()
                .message
                .contains("image trop volumineuse")
        );
    }

    #[test]
    fn the_image_bound_is_the_engine_limit() {
        assert_eq!(
            MAX_IMAGE_BYTES,
            bricks_agent::control::attach::AttachLimits::default().max_image_bytes
        );
    }
}
