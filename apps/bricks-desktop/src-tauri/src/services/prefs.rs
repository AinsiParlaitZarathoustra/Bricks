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
            Ok(bytes) => parse_fields(&bytes, &self.file),
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

/// Reads the preferences field by field. A value that does not fit its field
/// (an unknown `tool_approval`, a width written as text) takes that field's
/// default and is reported; the other fields are kept. Unknown keys are
/// ignored. A file that is not a JSON object gives the defaults.
fn parse_fields(bytes: &[u8], file: &std::path::Path) -> Preferences {
    let stored: serde_json::Value = match serde_json::from_slice(bytes) {
        Ok(value) => value,
        Err(e) => {
            eprintln!("bricks-desktop: {} ignored: {e}", file.display());
            return Preferences::default();
        }
    };
    let Some(stored) = stored.as_object() else {
        eprintln!("bricks-desktop: {} ignored: not an object", file.display());
        return Preferences::default();
    };
    let defaults = serde_json::to_value(Preferences::default())
        .ok()
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    let mut merged = defaults;
    for (key, value) in stored {
        if !merged.contains_key(key) {
            continue;
        }
        let mut candidate = merged.clone();
        candidate.insert(key.clone(), value.clone());
        if serde_json::from_value::<Preferences>(serde_json::Value::Object(candidate.clone()))
            .is_ok()
        {
            merged = candidate;
        } else {
            eprintln!(
                "bricks-desktop: {} : preference `{key}` ignored, its default is used",
                file.display()
            );
        }
    }
    serde_json::from_value(serde_json::Value::Object(merged)).unwrap_or_default()
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
    fn an_unknown_approval_value_keeps_every_other_preference() {
        use crate::contract::ToolApproval;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("preferences.json");
        std::fs::write(
            &file,
            r#"{"theme":"dark","left_width":300,"right_open":false,
                "last_project_id":"p_1","tool_approval":"yolo","right_width":"wide",
                "from_a_newer_version":true}"#,
        )
        .unwrap();
        let p = PrefsStore::at(file.clone()).get();
        assert!(matches!(p.theme, Theme::Dark), "kept");
        assert_eq!(p.left_width, 300, "kept");
        assert!(!p.right_open, "kept");
        assert_eq!(p.last_project_id.as_deref(), Some("p_1"), "kept");
        assert_eq!(
            p.tool_approval,
            ToolApproval::Standard,
            "unknown value: standard, never more"
        );
        assert_eq!(
            p.right_width, 280,
            "a value of the wrong type takes its default"
        );

        // A valid approval value is read as written.
        std::fs::write(&file, r#"{"tool_approval":"always_ask"}"#).unwrap();
        assert_eq!(
            PrefsStore::at(file).get().tool_approval,
            ToolApproval::AlwaysAsk
        );
    }

    #[test]
    fn a_damaged_file_gives_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("preferences.json");
        std::fs::write(&file, "[1,2").unwrap();
        assert_eq!(PrefsStore::at(file).get().right_width, 280);
    }
}
