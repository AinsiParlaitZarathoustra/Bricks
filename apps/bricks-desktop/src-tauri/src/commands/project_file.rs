//! Read-only windows of project files, for the file references of messages
//! (`path:line` or `path:line-end`), and the save dialog of SVG diagrams.
//!
//! `project_file_read` never reads outside the project folder (canonical
//! paths, symbolic links included), never reads more than `READ_LIMIT` bytes,
//! refuses binary and non UTF-8 content, and returns at most `WINDOW_LINES`
//! lines. No shell, no write. `save_svg_file` opens the native save dialog
//! here, in Rust, and writes only the file the user picked.

use crate::contract::{IpcError, IpcErrorCode, IpcResult, ProjectFileWindow};
use crate::engine::AppState;
use crate::git::{is_binary_content, validate_relative_path};
use crate::services::{invalid, io_error, join_error, not_found};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use tauri::State;
use tauri_plugin_dialog::DialogExt;

/// Lines in one window at most.
pub const WINDOW_LINES: u32 = 400;
/// Bytes read from one file at most; the rest of a larger file is not read.
pub const READ_LIMIT: u64 = 2 * 1024 * 1024;
/// Largest SVG document saved.
pub const SVG_LIMIT: usize = 5 * 1024 * 1024;

/// The window of `rel` (a path relative to the project folder `root`) asked
/// by `line` and `end_line` (1-based, inclusive). Without `line`, the first
/// lines. With `line` alone, `WINDOW_LINES` lines centred on it. With both, the
/// range `line..=end_line`: a window of `WINDOW_LINES` lines that holds it,
/// centred on its middle; a range as long as the window starts the window.
pub fn read_window(
    root: &Path,
    rel: &str,
    line: Option<u32>,
    end_line: Option<u32>,
) -> IpcResult<ProjectFileWindow> {
    validate_relative_path(rel)?;
    let root = std::fs::canonicalize(root).map_err(|e| IpcError {
        code: IpcErrorCode::FolderMissing,
        message: format!("le dossier du projet n'est plus accessible : {e}"),
    })?;
    let target = std::fs::canonicalize(root.join(rel)).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => not_found(format!("`{rel}` n'existe pas dans le projet")),
        _ => io_error(&format!("lecture de `{rel}`"), e),
    })?;
    if !target.starts_with(&root) {
        return Err(invalid(format!(
            "`{rel}` sort du projet : le lien symbolique est refusé"
        )));
    }
    let meta = std::fs::metadata(&target).map_err(|e| io_error(&format!("lecture de `{rel}`"), e))?;
    if !meta.is_file() {
        return Err(invalid(format!("`{rel}` n'est pas un fichier")));
    }

    let mut bytes = Vec::new();
    File::open(&target)
        .and_then(|file| file.take(READ_LIMIT + 1).read_to_end(&mut bytes))
        .map_err(|e| io_error(&format!("lecture de `{rel}`"), e))?;
    let mut truncated = bytes.len() as u64 > READ_LIMIT;
    if truncated {
        // Cut after the last newline: no half line, no half UTF-8 character.
        bytes.truncate(READ_LIMIT as usize);
        match bytes.iter().rposition(|b| *b == b'\n') {
            Some(pos) => bytes.truncate(pos + 1),
            None => bytes.clear(),
        }
    }
    if is_binary_content(&bytes) {
        return Err(invalid(format!(
            "`{rel}` est un fichier binaire : aperçu indisponible"
        )));
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| invalid(format!("`{rel}` n'est pas du texte UTF-8")))?;

    let mut lines: Vec<String> = text
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
        .collect();
    if text.is_empty() || text.ends_with('\n') {
        lines.pop();
    }
    let total = u32::try_from(lines.len()).unwrap_or(u32::MAX);

    let (first, last) = window_bounds(total, line, end_line)?;
    let window = if first <= last {
        lines[(first - 1) as usize..last as usize].to_vec()
    } else {
        Vec::new()
    };
    // The file is larger than the window: a range inside a short file is not truncated.
    truncated |= total > WINDOW_LINES;

    Ok(ProjectFileWindow {
        path: rel.to_string(),
        start: first,
        lines: window,
        total_lines: total,
        truncated,
        language: language_of(rel),
    })
}

/// First and last line (1-based, inclusive) of the window of a file of
/// `total` lines. An empty file gives `(1, 0)`.
fn window_bounds(total: u32, line: Option<u32>, end_line: Option<u32>) -> IpcResult<(u32, u32)> {
    match (line, end_line) {
        (None, Some(_)) => Err(invalid("une fin de plage sans ligne de début")),
        (None, None) => Ok((1, total.min(WINDOW_LINES))),
        (Some(0), _) => Err(invalid("les lignes commencent à 1")),
        (Some(line), end) => {
            if line > total {
                return Err(beyond_end(line, total));
            }
            let last = match end {
                None => line,
                Some(end) => {
                    if end < line {
                        return Err(invalid(format!(
                            "plage invalide : la fin ({end}) précède le début ({line})"
                        )));
                    }
                    end.min(total)
                }
            };
            if last - line + 1 >= WINDOW_LINES {
                // A range as long as the window: the window starts on it.
                return Ok((line, line.saturating_add(WINDOW_LINES - 1).min(total)));
            }
            Ok(centred_window(total, line, last))
        }
    }
}

/// The window of `WINDOW_LINES` lines that holds the range `line..=last`, which
/// is shorter than the window, and is centred on the middle of the range. Near
/// the start or the end of the file, it is pushed back into the file. A file of
/// at most `WINDOW_LINES` lines is returned whole.
fn centred_window(total: u32, line: u32, last: u32) -> (u32, u32) {
    if total <= WINDOW_LINES {
        return (1, total);
    }
    // The first line that keeps `last` in the window, the last first line that
    // keeps `line` in it, and the first line that centres the middle of the range.
    let lowest = last.saturating_add(1).saturating_sub(WINDOW_LINES).max(1);
    let highest = line.min(total + 1 - WINDOW_LINES);
    let middle = line + (last - line) / 2;
    let wanted = middle.saturating_sub(WINDOW_LINES / 2).max(1);
    let first = wanted.max(lowest).min(highest);
    (first, first + WINDOW_LINES - 1)
}

fn beyond_end(line: u32, total: u32) -> IpcError {
    invalid(format!(
        "ligne {line} au-delà de la fin du fichier ({total} lignes)"
    ))
}

/// Highlight.js language of a file name, from its extension (`None` when
/// unknown). The frontend still checks that the language is registered.
pub fn language_of(rel: &str) -> Option<String> {
    let name = Path::new(rel).file_name()?.to_str()?.to_ascii_lowercase();
    if name == "makefile" {
        return Some("makefile".into());
    }
    let (stem, ext) = name.rsplit_once('.')?;
    if stem.is_empty() {
        return None;
    }
    let language = match ext {
        "rs" => "rust",
        "ts" | "mts" | "cts" | "tsx" => "typescript",
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "json" => "json",
        "svelte" | "html" | "htm" | "vue" | "xml" | "svg" => "xml",
        "css" => "css",
        "scss" => "scss",
        "md" | "markdown" => "markdown",
        "py" => "python",
        "rb" => "ruby",
        "go" => "go",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "swift" => "swift",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" => "cpp",
        "cs" => "csharp",
        "php" => "php",
        "sh" | "bash" | "zsh" => "bash",
        "sql" => "sql",
        "yml" | "yaml" => "yaml",
        "toml" | "ini" | "cfg" => "ini",
        "diff" | "patch" => "diff",
        "lua" => "lua",
        _ => return None,
    };
    Some(language.into())
}

/// Reads the window of a file of the project `project_id` (see `read_window`).
#[tauri::command]
pub async fn project_file_read(
    state: State<'_, AppState>,
    project_id: String,
    path: String,
    line: Option<u32>,
    end_line: Option<u32>,
) -> IpcResult<ProjectFileWindow> {
    let root = state.project_path(&project_id)?;
    tokio::task::spawn_blocking(move || read_window(&root, &path, line, end_line))
        .await
        .map_err(join_error)?
}

/// What `save_svg_file` accepts: an SVG document of at most `SVG_LIMIT` bytes.
pub fn check_svg(contents: &str) -> IpcResult<()> {
    if contents.len() > SVG_LIMIT {
        return Err(invalid(
            "diagramme trop volumineux pour être enregistré (5 Mio au maximum)",
        ));
    }
    let head = contents.trim_start();
    if !(head.starts_with("<?xml") || head.starts_with("<svg")) {
        return Err(invalid("le contenu n'est pas un document SVG"));
    }
    Ok(())
}

/// The file name proposed by the save dialog: no folder, always `.svg`.
pub fn svg_file_name(requested: &str) -> String {
    let cleaned = requested.replace('\0', "");
    let base = Path::new(cleaned.trim())
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("diagramme");
    if base.to_ascii_lowercase().ends_with(".svg") {
        base.to_string()
    } else {
        format!("{base}.svg")
    }
}

/// Shows the native save dialog and writes `contents` to the file the user
/// picked. `None` when the dialog is cancelled; otherwise the path written.
#[tauri::command]
pub async fn save_svg_file(
    app: tauri::AppHandle,
    default_name: String,
    contents: String,
) -> IpcResult<Option<String>> {
    check_svg(&contents)?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("SVG", &["svg"])
        .set_file_name(svg_file_name(&default_name))
        .save_file(move |picked| {
            let _ = tx.send(picked);
        });
    let picked = rx.await.map_err(|_| IpcError {
        code: IpcErrorCode::Internal,
        message: "le dialogue d'enregistrement s'est fermé sans réponse".into(),
    })?;
    let Some(picked) = picked else {
        return Ok(None);
    };
    let path = picked
        .into_path()
        .map_err(|e| io_error("chemin d'enregistrement", e))?;
    let shown = path.display().to_string();
    tokio::task::spawn_blocking(move || std::fs::write(&path, contents.as_bytes()))
        .await
        .map_err(join_error)?
        .map_err(|e| io_error("écriture du diagramme", e))?;
    Ok(Some(shown))
}
