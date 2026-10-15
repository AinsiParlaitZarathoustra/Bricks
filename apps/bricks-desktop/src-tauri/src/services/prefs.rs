//! Window and display preferences, in `<bricks_home>/desktop/preferences.json`.

use super::paths::{desktop_dir, write_atomic};
use super::{error, io_error};
use crate::contract::{IpcErrorCode, IpcResult, Preferences};
use std::path::PathBuf;

const WIDTH: std::ops::RangeInclusive<u32> = 160..=640;
const TERMINAL_HEIGHT: std::ops::RangeInclusive<u32> = 120..=900;

pub struct PrefsStore {
    file: PathBuf,
}

impl PrefsStore {
    pub fn open() -> Self {
        Self {
            file: desktop_dir().join("preferences.json"),
        }
    }

    pub fn at(file: PathBuf) -> Self {
        Self { file }
    }

    /// The stored preferences. A missing or unreadable file gives the
    /// defaults; an unreadable one is reported on stderr and replaced by the
    /// next `set`, never silently merged.
    pub fn get(&self) -> Preferences {
        match std::fs::read(&self.file) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                eprintln!("bricks-desktop: {} ignored: {e}", self.file.display());
                Preferences::default()
            }),
            Err(_) => Preferences::default(),
        }
    }

    pub fn set(&self, prefs: Preferences) -> IpcResult<()> {
        let clean = sanitize(prefs);
        let bytes = serde_json::to_vec_pretty(&clean)
            .map_err(|e| error(IpcErrorCode::Internal, e.to_string()))?;
        write_atomic(&self.file, &bytes)
            .map_err(|e| io_error(&format!("préférences ({})", self.file.display()), e))
    }
}

/// Keeps the sizes inside what the interface can lay out.
fn sanitize(mut p: Preferences) -> Preferences {
    p.left_width = p.left_width.clamp(*WIDTH.start(), *WIDTH.end());
    p.right_width = p.right_width.clamp(*WIDTH.start(), *WIDTH.end());
    p.terminal_height = p
        .terminal_height
        .clamp(*TERMINAL_HEIGHT.start(), *TERMINAL_HEIGHT.end());
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::Theme;

    #[test]
    fn preferences_round_trip_and_are_clamped() {
        let dir = tempfile::tempdir().unwrap();
        let store = PrefsStore::at(dir.path().join("desktop").join("preferences.json"));
        assert_eq!(store.get().left_width, 260, "defaults when absent");
        let mut p = Preferences::default();
        p.theme = Theme::Dark;
        p.left_width = 9_000;
        p.terminal_height = 3;
        store.set(p).unwrap();
        let back = store.get();
        assert!(matches!(back.theme, Theme::Dark));
        assert_eq!(back.left_width, 640);
        assert_eq!(back.terminal_height, 120);
    }

    #[test]
    fn a_damaged_file_gives_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("preferences.json");
        std::fs::write(&file, "[1,2").unwrap();
        assert_eq!(PrefsStore::at(file).get().right_width, 280);
    }
}
