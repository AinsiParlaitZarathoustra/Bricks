//! Pasted or dropped files, stored as application-owned temporary files
//! under `<bricks_home>/desktop/attachments/` and used by a prompt.

use super::{error, invalid, io_error};
use crate::contract::{IpcErrorCode, IpcResult, SavedAttachment};
use base64::Engine;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Largest attachment accepted (decoded bytes).
pub const MAX_ATTACHMENT_BYTES: usize = 20 * 1024 * 1024;
/// Attachments older than this are deleted at startup.
pub const RETENTION: Duration = Duration::from_secs(7 * 24 * 3600);

/// `<bricks_home>/desktop/attachments`.
pub fn attachments_dir() -> PathBuf {
    super::paths::desktop_dir().join("attachments")
}

/// Decodes `data_base64` and stores it as `<dir>/<uuid>-<safe name>`.
/// Runs blocking I/O: call it from `spawn_blocking`.
pub fn save(dir: &Path, name: &str, media_type: &str, data_base64: &str) -> IpcResult<SavedAttachment> {
    let media_type = media_type.trim();
    if !valid_media_type(media_type) {
        return Err(invalid(format!("type de fichier invalide : {media_type}")));
    }
    // The payload may come as a data URL; only what follows the comma counts.
    let data = match data_base64.strip_prefix("data:") {
        Some(rest) => rest.split_once(',').map_or("", |(_, d)| d),
        None => data_base64,
    };
    if data.len() / 4 * 3 > MAX_ATTACHMENT_BYTES + 3 {
        return Err(too_large());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data.trim())
        .map_err(|e| invalid(format!("contenu illisible : {e}")))?;
    if bytes.len() > MAX_ATTACHMENT_BYTES {
        return Err(too_large());
    }
    std::fs::create_dir_all(dir).map_err(|e| io_error("pièces jointes", e))?;
    let path = dir.join(format!(
        "{}-{}",
        uuid::Uuid::new_v4().simple(),
        safe_name(name)
    ));
    std::fs::write(&path, &bytes).map_err(|e| io_error("pièce jointe", e))?;
    Ok(SavedAttachment {
        path: path.display().to_string(),
        media_type: media_type.to_string(),
        bytes: bytes.len() as u64,
    })
}

/// Deletes the files of `dir` last modified before `older_than`. Returns how
/// many were removed; failures are skipped (a file in use stays).
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

fn too_large() -> crate::contract::IpcError {
    error(
        IpcErrorCode::Invalid,
        format!(
            "fichier trop volumineux (maximum {} Mo)",
            MAX_ATTACHMENT_BYTES / (1024 * 1024)
        ),
    )
}

fn valid_media_type(m: &str) -> bool {
    let Some((kind, sub)) = m.split_once('/') else {
        return false;
    };
    let ok = |s: &str| {
        !s.is_empty()
            && s.len() <= 80
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-' | '_'))
    };
    ok(kind) && ok(sub)
}

/// The file name as the user knows it, without any path or character that
/// is unsafe in a file name.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[test]
    fn an_attachment_is_stored_under_its_safe_name() {
        let dir = tempfile::tempdir().unwrap();
        let saved = save(dir.path(), "../rap port.txt", "text/plain", &b64(b"abc")).unwrap();
        assert_eq!(saved.bytes, 3);
        assert_eq!(saved.media_type, "text/plain");
        let path = PathBuf::from(&saved.path);
        assert_eq!(path.parent().unwrap(), dir.path());
        assert!(path.file_name().unwrap().to_string_lossy().ends_with("-rap port.txt"));
        assert_eq!(std::fs::read(&path).unwrap(), b"abc");
    }

    #[test]
    fn oversized_or_malformed_payloads_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let big = vec![0u8; MAX_ATTACHMENT_BYTES + 1];
        assert_eq!(
            save(dir.path(), "a", "image/png", &b64(&big)).unwrap_err().code,
            IpcErrorCode::Invalid
        );
        assert_eq!(
            save(dir.path(), "a", "image/png", "%%%").unwrap_err().code,
            IpcErrorCode::Invalid
        );
        assert_eq!(
            save(dir.path(), "a", "not a type", &b64(b"x")).unwrap_err().code,
            IpcErrorCode::Invalid
        );
    }

    #[test]
    fn a_data_url_is_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let url = format!("data:image/png;base64,{}", b64(b"png"));
        assert_eq!(save(dir.path(), "x.png", "image/png", &url).unwrap().bytes, 3);
    }

    #[test]
    fn old_attachments_are_removed_and_recent_ones_kept() {
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
    fn names_are_made_safe() {
        assert_eq!(safe_name(""), "fichier");
        assert_eq!(safe_name(".."), "fichier");
        assert_eq!(safe_name("a/b/c:d.md"), "c_d.md");
    }
}
