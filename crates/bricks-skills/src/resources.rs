//! The files that come with a skill (`scripts/`, `references/`, `assets/`,
//! or any other name): listed when the skill is activated, never read or
//! run by this crate.
//!
//! A path is resolved from the skill's directory: absolute paths, `..`, and
//! symbolic links that lead outside the directory are refused; the check is
//! made on the canonical path at the moment of the request. Reading the
//! file is then the work of the ordinary tools (`Read`, with its
//! permissions); running a script is `Bash`'s, in the agent's own working
//! directory.

use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

/// Entries listed for one activation, at most.
pub const MAX_LISTED: usize = 64;
/// Depth of the listing below the skill's directory.
pub const MAX_DEPTH: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resource {
    /// Relative to the skill's directory, with `/`.
    pub path: String,
    pub bytes: u64,
    /// A symbolic link (listed, not followed).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub link: bool,
}

/// The files of a skill (not `SKILL.md`), sorted, bounded. `true` when the
/// listing stopped at a bound.
pub fn list(dir: &Path) -> (Vec<Resource>, bool) {
    let mut out = Vec::new();
    let mut truncated = false;
    walk(dir, dir, 0, &mut out, &mut truncated);
    out.sort_by(|a, b| a.path.cmp(&b.path));
    (out, truncated)
}

fn walk(base: &Path, dir: &Path, depth: usize, out: &mut Vec<Resource>, truncated: &mut bool) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if out.len() >= MAX_LISTED {
            *truncated = true;
            return;
        }
        let Ok(meta) = std::fs::symlink_metadata(&p) else {
            continue;
        };
        let rel = p
            .strip_prefix(base)
            .unwrap_or(&p)
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join("/");
        if depth == 0 && rel == "SKILL.md" {
            continue;
        }
        if rel.split('/').any(|part| part.starts_with('.')) {
            continue;
        }
        if meta.file_type().is_symlink() {
            out.push(Resource {
                path: rel,
                bytes: 0,
                link: true,
            });
        } else if meta.is_dir() {
            if depth + 1 < MAX_DEPTH {
                walk(base, &p, depth + 1, out, truncated);
            } else {
                *truncated = true;
            }
        } else if meta.is_file() {
            out.push(Resource {
                path: rel,
                bytes: meta.len(),
                link: false,
            });
        }
    }
}

/// Why a resource path is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceError(pub String);

impl std::fmt::Display for ResourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ResourceError {}

/// The canonical path of `rel` inside `dir`, checked now.
pub fn resolve(dir: &Path, rel: &str) -> Result<PathBuf, ResourceError> {
    let rel_path = Path::new(rel);
    if rel.trim().is_empty() {
        return Err(ResourceError("empty path".into()));
    }
    if rel_path.is_absolute() || rel.starts_with('~') {
        return Err(ResourceError(
            "absolute paths are refused: give a path relative to the skill's directory".into(),
        ));
    }
    for c in rel_path.components() {
        match c {
            Component::Normal(_) | Component::CurDir => {}
            _ => {
                return Err(ResourceError(
                    "`..` and root components are refused: the path must stay in the skill's \
                     directory"
                        .into(),
                ));
            }
        }
    }
    let base = std::fs::canonicalize(dir)
        .map_err(|e| ResourceError(format!("the skill's directory is not available: {e}")))?;
    let full = std::fs::canonicalize(base.join(rel_path))
        .map_err(|e| ResourceError(format!("`{rel}`: {e}")))?;
    if !full.starts_with(&base) {
        return Err(ResourceError(format!(
            "`{rel}` leads outside the skill's directory (symbolic link): refused"
        )));
    }
    Ok(full)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill() -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        std::fs::write(d.join("SKILL.md"), "x").unwrap();
        std::fs::create_dir_all(d.join("scripts")).unwrap();
        std::fs::write(d.join("scripts/run.sh"), "echo hi").unwrap();
        std::fs::create_dir_all(d.join("references")).unwrap();
        std::fs::write(d.join("references/guide.md"), "guide").unwrap();
        std::fs::create_dir_all(d.join("data/deep/deeper/deepest")).unwrap();
        std::fs::write(d.join("data/deep/deeper/deepest/x.txt"), "x").unwrap();
        std::fs::write(d.join(".hidden"), "x").unwrap();
        t
    }

    #[test]
    fn listing_is_relative_sorted_and_bounded_in_depth() {
        let t = skill();
        let (list, truncated) = list(t.path());
        let paths: Vec<&str> = list.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(paths, ["references/guide.md", "scripts/run.sh"]);
        assert!(truncated, "the deepest folder is beyond the depth");
        assert_eq!(list[1].bytes, 7);
    }

    #[test]
    fn traversal_and_absolute_paths_are_refused() {
        let t = skill();
        assert!(resolve(t.path(), "scripts/run.sh").is_ok());
        assert!(resolve(t.path(), "./references/guide.md").is_ok());
        for bad in ["../x", "scripts/../../x", "/etc/passwd", "~/x", ""] {
            assert!(resolve(t.path(), bad).is_err(), "{bad}");
        }
        assert!(resolve(t.path(), "missing.txt").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn links_leaving_the_directory_are_refused() {
        let t = skill();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "s").unwrap();
        std::os::unix::fs::symlink(outside.path(), t.path().join("out")).unwrap();
        std::os::unix::fs::symlink(
            t.path().join("references/guide.md"),
            t.path().join("inside-link.md"),
        )
        .unwrap();
        assert!(resolve(t.path(), "out/secret.txt").is_err());
        assert!(resolve(t.path(), "inside-link.md").is_ok());
        let (list, _) = list(t.path());
        assert!(list.iter().any(|r| r.path == "out" && r.link));
    }
}
