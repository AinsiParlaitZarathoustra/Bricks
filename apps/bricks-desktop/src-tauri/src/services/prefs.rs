//! Window and display preferences, in `<bricks_home>/desktop/preferences.json`.

use super::paths::{desktop_dir, write_atomic};
use super::{error, io_error};
use crate::contract::{ColorSet, IpcErrorCode, IpcResult, Preferences, ThemeColors};
use std::path::PathBuf;

const WIDTH: std::ops::RangeInclusive<u32> = 160..=640;
const TERMINAL_HEIGHT: std::ops::RangeInclusive<u32> = 120..=900;
/// Share of the centre column the terminal pane may take.
const TERMINAL_RATIO: std::ops::RangeInclusive<f32> = 0.1..=0.8;

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
            Ok(bytes) => sanitize(parse_fields(&bytes, &self.file)),
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
/// (a width written as text) takes that field's
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

/// Keeps the values inside what the interface can lay out or apply.
///
/// * sizes are clamped to their ranges;
/// * each colour must be `#rrggbb` in lower case: any other value becomes
///   `null` (the theme's token), the other colours are kept;
/// * `terminal_ratio` is clamped to 0.1–0.8; a value that is not a finite
///   number becomes `null`. A file that has no `terminal_ratio` keeps `null`
///   even when it has a legacy `terminal_height`: the interface derives the
///   ratio from it once, then saves it here.
fn sanitize(mut p: Preferences) -> Preferences {
    // These capabilities cannot be enabled by editing the preferences file or IPC.
    p.diagrams.diagram_viewer.allow_links = false;
    p.diagrams.diagram_viewer.allow_animations = false;
    p.diagrams.diagram_viewer.allow_edit = false;
    p.diagrams.diagram_viewer.allow_java_script = false;
    p.left_width = p.left_width.clamp(*WIDTH.start(), *WIDTH.end());
    p.right_width = p.right_width.clamp(*WIDTH.start(), *WIDTH.end());
    p.terminal_height = p
        .terminal_height
        .clamp(*TERMINAL_HEIGHT.start(), *TERMINAL_HEIGHT.end());
    // A custom duration stays within what the engine accepts.
    p.ask_timer.seconds = p.ask_timer.seconds.clamp(30, 3_600);
    p.colors = ThemeColors {
        light: sanitize_colors(p.colors.light),
        dark: sanitize_colors(p.colors.dark),
    };
    p.terminal_ratio = p
        .terminal_ratio
        .filter(|r| r.is_finite())
        .map(|r| r.clamp(*TERMINAL_RATIO.start(), *TERMINAL_RATIO.end()));
    p
}

fn sanitize_colors(c: ColorSet) -> ColorSet {
    ColorSet {
        accent: c.accent.filter(|v| is_colour(v)),
        background: c.background.filter(|v| is_colour(v)),
        panel: c.panel.filter(|v| is_colour(v)),
    }
}

/// `#rrggbb`, lower-case hexadecimal digits only.
fn is_colour(value: &str) -> bool {
    let b = value.as_bytes();
    b.len() == 7 && b[0] == b'#' && b[1..].iter().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{AskTimerMode, AskTimerPrefs, Theme};

    #[test]
    fn the_app_icon_follows_the_theme_by_default_and_keeps_a_choice() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("preferences.json");
        std::fs::write(&file, r#"{"theme":"dark"}"#).unwrap();
        let store = PrefsStore::at(file.clone());
        let mut p = store.get();
        assert_eq!(p.app_icon, crate::contract::AppIcon::Aligned, "an older file");
        p.app_icon = crate::contract::AppIcon::Dark;
        store.set(p).unwrap();
        assert_eq!(PrefsStore::at(file).get().app_icon, crate::contract::AppIcon::Dark);
    }

    #[test]
    fn preferences_round_trip_and_are_clamped() {
        let dir = tempfile::tempdir().unwrap();
        let store = PrefsStore::at(dir.path().join("desktop").join("preferences.json"));
        assert_eq!(store.get().left_width, 260, "defaults when absent");
        let p = Preferences {
            theme: Theme::Dark,
            left_width: 9_000,
            terminal_height: 3,
            ..Preferences::default()
        };
        store.set(p).unwrap();
        let back = store.get();
        assert!(matches!(back.theme, Theme::Dark));
        assert_eq!(back.left_width, 640);
        assert_eq!(back.terminal_height, 120);
    }

    #[test]
    fn a_wrong_value_keeps_every_other_preference() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("preferences.json");
        std::fs::write(
            &file,
            r#"{"theme":"dark","left_width":300,"right_open":false,
                "last_project_id":"p_1","tool_approval":"no_confirmation","right_width":"wide",
                "from_a_newer_version":true}"#,
        )
        .unwrap();
        let p = PrefsStore::at(file.clone()).get();
        assert!(matches!(p.theme, Theme::Dark), "kept");
        assert_eq!(p.left_width, 300, "kept");
        assert!(!p.right_open, "kept");
        assert_eq!(p.last_project_id.as_deref(), Some("p_1"), "kept");
        assert_eq!(
            p.right_width, 280,
            "a value of the wrong type takes its default"
        );
        // The old approval preference is no longer read nor written back.
        PrefsStore::at(file.clone()).set(p).unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert!(saved.get("tool_approval").is_none());
    }

    #[test]
    fn a_damaged_file_gives_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("preferences.json");
        std::fs::write(&file, "[1,2").unwrap();
        assert_eq!(PrefsStore::at(file).get().right_width, 280);
    }

    #[test]
    fn colours_and_the_terminal_ratio_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = PrefsStore::at(dir.path().join("preferences.json"));
        let mut p = Preferences::default();
        p.colors.dark.accent = Some("#3366ff".into());
        p.colors.light.background = Some("#fafafa".into());
        p.terminal_ratio = Some(0.35);
        store.set(p).unwrap();
        let back = store.get();
        assert_eq!(back.colors.dark.accent.as_deref(), Some("#3366ff"));
        assert_eq!(back.colors.dark.panel, None, "unset stays a theme token");
        assert_eq!(back.colors.light.background.as_deref(), Some("#fafafa"));
        assert_eq!(back.terminal_ratio, Some(0.35));
    }

    #[test]
    fn an_invalid_colour_becomes_null_and_the_others_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("preferences.json");
        std::fs::write(
            &file,
            r##"{"colors":{
                "light":{"accent":"#A1B2C3","background":"#ffffff","panel":"red"},
                "dark":{"accent":"#0a0a0a","background":"#12345","panel":"#ggg000"}}}"##,
        )
        .unwrap();
        let c = PrefsStore::at(file.clone()).get().colors;
        assert_eq!(c.light.accent, None, "upper case is not the stored form");
        assert_eq!(c.light.background.as_deref(), Some("#ffffff"), "kept");
        assert_eq!(c.light.panel, None, "a colour name is refused");
        assert_eq!(c.dark.accent.as_deref(), Some("#0a0a0a"), "kept");
        assert_eq!(c.dark.background, None, "five digits");
        assert_eq!(c.dark.panel, None, "not hexadecimal");

        // Saving applies the same rule, so the file holds only valid values.
        let p = PrefsStore::at(file.clone()).get();
        PrefsStore::at(file.clone()).set(p).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(!text.contains("#A1B2C3") && !text.contains("red"), "{text}");
    }

    #[test]
    fn the_terminal_ratio_is_clamped_and_a_missing_one_stays_unset() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("preferences.json");
        std::fs::write(&file, r#"{"terminal_ratio":0.95}"#).unwrap();
        assert_eq!(PrefsStore::at(file.clone()).get().terminal_ratio, Some(0.8));
        std::fs::write(&file, r#"{"terminal_ratio":0.01}"#).unwrap();
        assert_eq!(PrefsStore::at(file.clone()).get().terminal_ratio, Some(0.1));
        std::fs::write(&file, r#"{"terminal_ratio":"half"}"#).unwrap();
        assert_eq!(PrefsStore::at(file.clone()).get().terminal_ratio, None, "wrong type");

        // A legacy file: the pixel height is kept, the ratio is left to the interface.
        std::fs::write(&file, r#"{"terminal_height":300,"terminal_open":true}"#).unwrap();
        let legacy = PrefsStore::at(file).get();
        assert_eq!(legacy.terminal_height, 300, "the legacy value is kept");
        assert!(legacy.terminal_open);
        assert_eq!(legacy.terminal_ratio, None, "derived once by the interface");
    }

    #[test]
    fn a_non_finite_or_out_of_range_ratio_is_clamped_or_dropped() {
        let ratio = |r: f32| {
            let p = Preferences {
                terminal_ratio: Some(r),
                ..Preferences::default()
            };
            sanitize(p).terminal_ratio
        };
        assert_eq!(ratio(f32::NAN), None);
        assert_eq!(ratio(f32::INFINITY), None);
        assert_eq!(ratio(f32::NEG_INFINITY), None);
        assert_eq!(ratio(5.0), Some(0.8));
        assert_eq!(ratio(-3.0), Some(0.1));
        assert_eq!(ratio(0.5), Some(0.5), "inside the range: kept");
    }

    #[test]
    fn an_old_file_without_the_new_fields_reads_as_before() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("preferences.json");
        std::fs::write(
            &file,
            r#"{"theme":"dark","left_width":300,"tool_approval":"always_ask","last_session_id":"s_9"}"#,
        )
        .unwrap();
        let p = PrefsStore::at(file.clone()).get();
        assert!(matches!(p.theme, crate::contract::Theme::Dark));
        assert_eq!(p.left_width, 300);
        assert_eq!(p.last_session_id.as_deref(), Some("s_9"));
        assert_eq!(p.colors, ThemeColors::default());
        assert_eq!(p.terminal_ratio, None);
        // Saving it adds the fields with their defaults, nothing else changes.
        PrefsStore::at(file.clone()).set(p).unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(saved["left_width"], 300);
        assert_eq!(saved["terminal_ratio"], serde_json::Value::Null);
        assert_eq!(saved["colors"]["light"]["accent"], serde_json::Value::Null);
    }
    #[test]
    fn diagrams_default_to_the_requested_sandbox_configuration() {
        let value = serde_json::to_value(Preferences::default().diagrams).unwrap();
        assert_eq!(
            value["mermaid"],
            serde_json::json!({"securityLevel":"sandbox","theme":"forest","look":"classic"})
        );
        assert_eq!(
            value["diagramViewer"],
            serde_json::json!({
                "followAppTheme":true,"allowZoom":true,"allowPan":true,"allowFullscreen":true,
                "allowDownload":true,"allowCopy":true,"allowTooltips":true,"allowLinks":false,
                "allowSelection":false,"allowAnimations":false,"allowEdit":false,"allowJavaScript":false
            })
        );
    }

    #[test]
    fn diagram_preferences_round_trip_but_cannot_enable_active_content() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("preferences.json");
        let mut prefs = Preferences::default();
        prefs.diagrams.mermaid.theme = crate::contract::MermaidTheme::Dark;
        prefs.diagrams.mermaid.security_level = crate::contract::MermaidSecurityLevel::Loose;
        let viewer = &mut prefs.diagrams.diagram_viewer;
        viewer.allow_zoom = false;
        viewer.allow_selection = true;
        viewer.allow_links = true;
        viewer.allow_java_script = true;
        viewer.allow_animations = true;
        viewer.allow_edit = true;
        PrefsStore::at(file.clone()).set(prefs).unwrap();
        let saved = PrefsStore::at(file).get();
        assert_eq!(
            saved.diagrams.mermaid.security_level,
            crate::contract::MermaidSecurityLevel::Loose
        );
        assert!(matches!(
            saved.diagrams.mermaid.theme,
            crate::contract::MermaidTheme::Dark
        ));
        let viewer = saved.diagrams.diagram_viewer;
        assert!(!viewer.allow_zoom);
        assert!(viewer.allow_selection);
        assert!(
            !viewer.allow_java_script
                && !viewer.allow_links
                && !viewer.allow_animations
                && !viewer.allow_edit
        );
    }

    #[test]
    fn unknown_diagram_themes_fall_back_without_losing_other_preferences() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("preferences.json");
        std::fs::write(
            &file,
            r#"{"left_width":310,"diagrams":{"mermaid":{"theme":"invented"}}}"#,
        )
        .unwrap();
        let prefs = PrefsStore::at(file).get();
        assert_eq!(prefs.left_width, 310);
        assert!(matches!(
            prefs.diagrams.mermaid.theme,
            crate::contract::MermaidTheme::Forest
        ));
    }

    #[test]
    fn the_question_timer_is_off_by_default_and_smart_when_enabled() {
        assert_eq!(
            Preferences::default().ask_timer,
            AskTimerPrefs {
                enabled: false,
                mode: AskTimerMode::Smart,
                seconds: 300,
            }
        );
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("preferences.json");
        std::fs::write(&file, r#"{"left_width":300}"#).unwrap();
        assert_eq!(
            PrefsStore::at(file).get().ask_timer,
            AskTimerPrefs::default(),
            "a file from before the question timer keeps it off"
        );
    }

    #[test]
    fn the_question_timer_seconds_are_kept_between_30_and_3600() {
        let seconds = |s: u32| {
            let p = Preferences {
                ask_timer: AskTimerPrefs {
                    enabled: true,
                    mode: AskTimerMode::Custom,
                    seconds: s,
                },
                ..Preferences::default()
            };
            sanitize(p).ask_timer.seconds
        };
        assert_eq!(seconds(0), 30);
        assert_eq!(seconds(29), 30);
        assert_eq!(seconds(30), 30);
        assert_eq!(seconds(300), 300);
        assert_eq!(seconds(3_600), 3_600);
        assert_eq!(seconds(3_601), 3_600);
        assert_eq!(seconds(u32::MAX), 3_600);
    }

    #[test]
    fn a_stored_question_timer_is_clamped_when_read_and_when_saved() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("preferences.json");
        std::fs::write(
            &file,
            r#"{"ask_timer":{"enabled":true,"mode":"custom","seconds":5}}"#,
        )
        .unwrap();
        let store = PrefsStore::at(file.clone());
        let p = store.get();
        assert_eq!(
            p.ask_timer,
            AskTimerPrefs {
                enabled: true,
                mode: AskTimerMode::Custom,
                seconds: 30,
            }
        );
        store.set(p).unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(saved["ask_timer"]["seconds"], 30);
        assert_eq!(saved["ask_timer"]["mode"], "custom");
    }

    #[test]
    fn the_question_timer_keeps_the_wire_shape_and_an_unknown_mode_falls_back() {
        let wire = serde_json::to_value(AskTimerPrefs {
            enabled: true,
            mode: AskTimerMode::Custom,
            seconds: 120,
        })
        .unwrap();
        assert_eq!(
            wire,
            serde_json::json!({"enabled": true, "mode": "custom", "seconds": 120})
        );

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("preferences.json");
        std::fs::write(
            &file,
            r#"{"left_width":310,"ask_timer":{"enabled":true,"mode":"invented","seconds":60}}"#,
        )
        .unwrap();
        let p = PrefsStore::at(file).get();
        assert_eq!(p.ask_timer, AskTimerPrefs::default(), "an unknown mode is ignored");
        assert_eq!(p.left_width, 310, "the other preferences are kept");
    }
}
