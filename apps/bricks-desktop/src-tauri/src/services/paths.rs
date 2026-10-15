//! The application's own folder (`<bricks_home>/desktop`) and the file
//! primitives every service shares.

use std::io::Write;
use std::path::{Path, PathBuf};

/// `<bricks_home>/desktop`: projects, preferences, change journal,
/// attachments.
pub fn desktop_dir() -> PathBuf {
    bricks_config::bricks_home().join("desktop")
}

/// `Ok` when `path` is a folder that can be listed; otherwise the reason.
pub fn check_folder(path: &Path) -> Result<(), String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_dir() {
        return Err("ce n'est pas un dossier".into());
    }
    std::fs::read_dir(path)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Writes `bytes` to `path` atomically: a temporary file in the same folder,
/// synced, then renamed over the target. A reader sees the old or the new
/// content, never a partial one.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("no parent folder"))?;
    std::fs::create_dir_all(dir)?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");
    let tmp = dir.join(format!(".{name}.{}.tmp", uuid::Uuid::new_v4().simple()));
    let written = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_atomic_write_replaces_the_whole_file_and_leaves_no_temporary() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sub").join("data.json");
        write_atomic(&file, b"one").unwrap();
        write_atomic(&file, b"two").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"two");
        let names: Vec<String> = std::fs::read_dir(file.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["data.json".to_string()]);
    }

    #[test]
    fn a_folder_check_names_what_is_wrong() {
        let dir = tempfile::tempdir().unwrap();
        assert!(check_folder(dir.path()).is_ok());
        let file = dir.path().join("f.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(check_folder(&file).unwrap_err().contains("dossier"));
        assert!(check_folder(&dir.path().join("absent")).is_err());
    }
}
