//! The profile registry: project > user > built-in.
//!
//! * Project profiles are `<workspace>/.bricks/agents/*.md`, where the
//!   workspace is the session's working directory (the folder whose
//!   `bricks.toml` applies); parent folders are not searched.
//! * User profiles are `~/.bricks/agents/*.md`.
//! * Built-in profiles are compiled in and parsed by the same parser.
//!
//! A higher scope shadows a lower one by name, **even when it is invalid**:
//! a broken project profile makes that name unavailable with its
//! diagnostic, it is never silently replaced by the built-in. Two files with
//! the same name in one scope make that name unavailable too. Other profiles
//! stay usable.
//!
//! A registry is an immutable snapshot; [`ProfileCatalog::reload`] makes a
//! new one. Instances keep the profile they were started with.

use super::profile::{
    intended_name, parse_profile, revision_of, AgentProfile, ProfileScope, ProfileSource,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Files read per scope directory.
pub const MAX_PROFILES_PER_SCOPE: usize = 256;

/// The built-in profiles (`crates/cersei-agent/agents/*.md`).
pub const BUILT_IN: &[(&str, &str)] = &[
    (
        "orchestrateur.md",
        include_str!("../../agents/orchestrateur.md"),
    ),
    (
        "web_searcher.md",
        include_str!("../../agents/web_searcher.md"),
    ),
    ("inspecteur.md", include_str!("../../agents/inspecteur.md")),
    (
        "backend_coder.md",
        include_str!("../../agents/backend_coder.md"),
    ),
    (
        "frontend_coder.md",
        include_str!("../../agents/frontend_coder.md"),
    ),
    ("testeur.md", include_str!("../../agents/testeur.md")),
    ("redactor.md", include_str!("../../agents/redactor.md")),
];

/// A file found in a scope folder.
enum Found {
    Text {
        label: String,
        path: Option<PathBuf>,
        text: String,
    },
    TooLarge {
        label: String,
        path: PathBuf,
        size: u64,
    },
}

/// Where to look.
#[derive(Debug, Clone, Default)]
pub struct ProfileSources {
    pub project_dir: Option<PathBuf>,
    pub user_dir: Option<PathBuf>,
}

impl ProfileSources {
    /// `<workspace>/.bricks/agents` and `~/.bricks/agents`.
    pub fn standard(workspace: &Path) -> Self {
        Self {
            project_dir: Some(workspace.join(".bricks").join("agents")),
            user_dir: dirs::home_dir().map(|h| h.join(".bricks").join("agents")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileDiagnostic {
    /// File (or `builtin:x.md`).
    pub source: String,
    pub message: String,
}

#[derive(Debug, Clone)]
enum Entry {
    Valid(Arc<AgentProfile>),
    Invalid { source: String, error: String },
}

#[derive(Debug, Clone)]
struct Slot {
    scope: ProfileScope,
    entry: Entry,
    /// Lower scopes defining the same name.
    shadows: Vec<(ProfileScope, String)>,
}

/// One line of the catalogue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileListing {
    pub name: String,
    pub description: String,
    pub scope: ProfileScope,
    pub source: String,
    /// Usable (`false`: see `error`).
    pub valid: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Lower-priority definitions of this name, hidden by this one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shadows: Vec<String>,
}

/// An immutable snapshot of the profiles.
#[derive(Debug, Clone, Default)]
pub struct ProfileRegistry {
    slots: BTreeMap<String, Slot>,
    pub diagnostics: Vec<ProfileDiagnostic>,
}

impl ProfileRegistry {
    /// Built-ins, then user and project profiles over them.
    pub fn load(sources: &ProfileSources) -> Self {
        let mut reg = Self::default();
        let builtins: Vec<Found> = BUILT_IN
            .iter()
            .map(|(f, t)| Found::Text {
                label: format!("builtin:{f}"),
                path: None,
                text: t.to_string(),
            })
            .collect();
        reg.add_scope(ProfileScope::BuiltIn, builtins);
        if let Some(d) = &sources.user_dir {
            let files = reg.read_dir(d);
            reg.add_scope(ProfileScope::User, files);
        }
        if let Some(d) = &sources.project_dir {
            let files = reg.read_dir(d);
            reg.add_scope(ProfileScope::Project, files);
        }
        reg
    }

    /// Only the built-in profiles.
    pub fn built_in() -> Self {
        Self::load(&ProfileSources::default())
    }

    fn read_dir(&mut self, dir: &Path) -> Vec<Found> {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return Vec::new(); // no folder: nothing defined there
        };
        let mut paths: Vec<PathBuf> = rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "md") && p.is_file())
            .collect();
        paths.sort();
        if paths.len() > MAX_PROFILES_PER_SCOPE {
            self.diagnostics.push(ProfileDiagnostic {
                source: dir.display().to_string(),
                message: format!(
                    "{} profile files: only the first {MAX_PROFILES_PER_SCOPE} (by name) are read",
                    paths.len()
                ),
            });
            paths.truncate(MAX_PROFILES_PER_SCOPE);
        }
        let mut out = Vec::new();
        for p in paths {
            let label = p.display().to_string();
            let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            if size as usize > super::profile::MAX_PROFILE_BYTES {
                // Still shadows its name: never fall back silently.
                out.push(Found::TooLarge {
                    label,
                    path: p,
                    size,
                });
                continue;
            }
            match std::fs::read_to_string(&p) {
                Ok(text) => out.push(Found::Text {
                    label,
                    path: Some(p),
                    text,
                }),
                Err(e) => self.diagnostics.push(ProfileDiagnostic {
                    source: label,
                    message: format!("cannot read: {e}"),
                }),
            }
        }
        out
    }

    fn add_scope(&mut self, scope: ProfileScope, files: Vec<Found>) {
        let mut here: BTreeMap<String, Vec<(String, Entry)>> = BTreeMap::new();
        for found in files {
            let (label, name, entry) = match found {
                Found::TooLarge { label, path, size } => (
                    label.clone(),
                    stem_of(Some(&path), &label),
                    Entry::Invalid {
                        source: label,
                        error: format!(
                            "{size} bytes: profiles are limited to {} bytes",
                            super::profile::MAX_PROFILE_BYTES
                        ),
                    },
                ),
                Found::Text { label, path, text } => {
                    let stem = stem_of(path.as_deref(), &label);
                    let source = ProfileSource {
                        scope,
                        path,
                        label: label.clone(),
                        revision: revision_of(&text),
                    };
                    match parse_profile(&text, source) {
                        Ok(p) => (label, p.name.clone(), Entry::Valid(Arc::new(p))),
                        Err(e) => (
                            label.clone(),
                            intended_name(&text, &stem),
                            Entry::Invalid {
                                source: label,
                                error: e,
                            },
                        ),
                    }
                }
            };
            if let Entry::Invalid { source, error } = &entry {
                self.diagnostics.push(ProfileDiagnostic {
                    source: source.clone(),
                    message: format!("profile `{name}` is invalid: {error}"),
                });
            }
            here.entry(name).or_default().push((label, entry));
        }
        for (name, mut defs) in here {
            let entry = if defs.len() > 1 {
                let labels: Vec<String> = defs.iter().map(|(l, _)| l.clone()).collect();
                let error = format!(
                    "defined {} times in the {} scope ({}): none is used",
                    defs.len(),
                    scope.as_str(),
                    labels.join(", ")
                );
                self.diagnostics.push(ProfileDiagnostic {
                    source: labels.join(", "),
                    message: format!("profile `{name}`: {error}"),
                });
                Entry::Invalid {
                    source: labels.join(", "),
                    error,
                }
            } else {
                defs.remove(0).1
            };
            let mut shadows = Vec::new();
            if let Some(lower) = self.slots.remove(&name) {
                shadows.push((lower.scope, slot_label(&lower)));
                shadows.extend(lower.shadows);
            }
            self.slots.insert(
                name,
                Slot {
                    scope,
                    entry,
                    shadows,
                },
            );
        }
    }

    /// A usable profile, or why not.
    pub fn get(&self, name: &str) -> Result<Arc<AgentProfile>, String> {
        match self.slots.get(name) {
            Some(Slot {
                entry: Entry::Valid(p),
                ..
            }) => Ok(Arc::clone(p)),
            Some(Slot {
                entry: Entry::Invalid { source, error },
                scope,
                ..
            }) => Err(format!(
                "profile `{name}` ({} scope, {source}) is invalid: {error}",
                scope.as_str()
            )),
            None => {
                let known: Vec<&str> = self
                    .slots
                    .iter()
                    .filter(|(_, s)| matches!(s.entry, Entry::Valid(_)))
                    .map(|(n, _)| n.as_str())
                    .take(40)
                    .collect();
                Err(format!(
                    "no profile `{name}`; available: {}",
                    if known.is_empty() {
                        "(none)".to_string()
                    } else {
                        known.join(", ")
                    }
                ))
            }
        }
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Every name, by name.
    pub fn list(&self) -> Vec<ProfileListing> {
        self.slots
            .iter()
            .map(|(name, s)| ProfileListing {
                name: name.clone(),
                description: match &s.entry {
                    Entry::Valid(p) => p.description.clone(),
                    Entry::Invalid { .. } => String::new(),
                },
                scope: s.scope,
                source: slot_label(s),
                valid: matches!(s.entry, Entry::Valid(_)),
                error: match &s.entry {
                    Entry::Invalid { error, .. } => Some(error.clone()),
                    Entry::Valid(_) => None,
                },
                shadows: s
                    .shadows
                    .iter()
                    .map(|(sc, l)| format!("{} ({l})", sc.as_str()))
                    .collect(),
            })
            .collect()
    }

    /// Names and descriptions matching `query` (case-insensitive substring;
    /// all when empty), one page of `per_page`. Returns the page and the
    /// number of matches.
    pub fn search(
        &self,
        query: &str,
        page: usize,
        per_page: usize,
    ) -> (Vec<ProfileListing>, usize) {
        let q = query.trim().to_lowercase();
        let all: Vec<ProfileListing> = self
            .list()
            .into_iter()
            .filter(|l| {
                q.is_empty()
                    || l.name.to_lowercase().contains(&q)
                    || l.description.to_lowercase().contains(&q)
            })
            .collect();
        let total = all.len();
        let per = per_page.clamp(1, 100);
        (all.into_iter().skip(page * per).take(per).collect(), total)
    }
}

fn stem_of(path: Option<&Path>, label: &str) -> String {
    path.and_then(|p| p.file_stem())
        .and_then(|s| s.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| {
            label
                .trim_start_matches("builtin:")
                .trim_end_matches(".md")
                .to_string()
        })
}

fn slot_label(s: &Slot) -> String {
    match &s.entry {
        Entry::Valid(p) => p.source.label.clone(),
        Entry::Invalid { source, .. } => source.clone(),
    }
}

/// The registry of a session, reloadable.
pub struct ProfileCatalog {
    sources: ProfileSources,
    current: parking_lot::RwLock<Arc<ProfileRegistry>>,
}

impl ProfileCatalog {
    pub fn new(sources: ProfileSources) -> Self {
        let reg = ProfileRegistry::load(&sources);
        Self {
            sources,
            current: parking_lot::RwLock::new(Arc::new(reg)),
        }
    }

    /// The current snapshot.
    pub fn snapshot(&self) -> Arc<ProfileRegistry> {
        Arc::clone(&self.current.read())
    }

    /// Read the files again. Running instances keep their profiles.
    pub fn reload(&self) -> Arc<ProfileRegistry> {
        let reg = Arc::new(ProfileRegistry::load(&self.sources));
        *self.current.write() = Arc::clone(&reg);
        reg
    }

    pub fn sources(&self) -> &ProfileSources {
        &self.sources
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(name: &str, desc: &str) -> String {
        format!("---\nname: {name}\ndescription: {desc}\n---\nInstructions of {name} ({desc}).\n")
    }

    #[test]
    fn the_seven_built_ins_load_with_the_same_parser() {
        let reg = ProfileRegistry::built_in();
        assert!(reg.diagnostics.is_empty(), "{:?}", reg.diagnostics);
        let names: Vec<String> = reg.list().into_iter().map(|l| l.name).collect();
        assert_eq!(
            names,
            vec![
                "backend_coder",
                "frontend_coder",
                "inspecteur",
                "orchestrateur",
                "redactor",
                "testeur",
                "web_searcher"
            ]
        );
        for n in &names {
            let p = reg.get(n).unwrap();
            assert_eq!(p.model, super::super::profile::ModelPref::Inherit);
            assert_eq!(p.permissions, "inherit");
            assert_eq!(p.tools, "inherit");
            assert!(!p.background);
            assert_eq!(p.isolation, super::super::profile::Isolation::Auto);
            assert!(p.max_turns.is_none(), "no business quota in {n}");
            assert!(p.skills.is_empty(), "no hypothetical skill in {n}");
            // No made-up tool: CodeScout is a real tool, named as such.
            for word in ["codescout", "Codescout"] {
                assert!(!p.instructions.contains(word), "{n}");
            }
        }
        assert!(reg
            .get("inspecteur")
            .unwrap()
            .instructions
            .contains("CodeScout"));
    }

    #[test]
    fn scopes_shadow_by_priority_with_visible_sources() {
        let d = tempfile::tempdir().unwrap();
        let (proj, user) = (d.path().join("p"), d.path().join("u"));
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::create_dir_all(&user).unwrap();
        std::fs::write(user.join("testeur.md"), profile("testeur", "user tester")).unwrap();
        std::fs::write(proj.join("t.md"), profile("testeur", "project tester")).unwrap();
        std::fs::write(user.join("mine.md"), profile("mine", "mine")).unwrap();
        let reg = ProfileRegistry::load(&ProfileSources {
            project_dir: Some(proj),
            user_dir: Some(user),
        });
        let t = reg.get("testeur").unwrap();
        assert_eq!(t.description, "project tester");
        assert_eq!(t.source.scope, ProfileScope::Project);
        let l = reg
            .list()
            .into_iter()
            .find(|l| l.name == "testeur")
            .unwrap();
        assert_eq!(l.shadows.len(), 2, "{:?}", l.shadows);
        assert!(l.shadows[0].starts_with("user"));
        assert!(l.shadows[1].starts_with("built_in"));
        assert_eq!(reg.get("mine").unwrap().source.scope, ProfileScope::User);
        assert_eq!(reg.len(), 8);
    }

    #[test]
    fn an_invalid_or_duplicated_profile_is_never_replaced_silently() {
        let d = tempfile::tempdir().unwrap();
        let proj = d.path().join("p");
        std::fs::create_dir_all(&proj).unwrap();
        // Invalid project override of a built-in.
        std::fs::write(
            proj.join("inspecteur.md"),
            "---\nname: inspecteur\ndescription: d\npermissions: everything\n---\n",
        )
        .unwrap();
        // Two files, one name.
        std::fs::write(proj.join("a.md"), profile("twin", "one")).unwrap();
        std::fs::write(proj.join("b.md"), profile("twin", "two")).unwrap();
        // Garbage file named by its stem.
        std::fs::write(proj.join("broken.md"), "not a profile").unwrap();
        std::fs::write(proj.join("ok.md"), profile("ok", "fine")).unwrap();
        let reg = ProfileRegistry::load(&ProfileSources {
            project_dir: Some(proj),
            user_dir: None,
        });
        let e = reg.get("inspecteur").unwrap_err();
        assert!(
            e.contains("project scope") && e.contains("only `inherit`"),
            "{e}"
        );
        assert!(reg.get("twin").unwrap_err().contains("defined 2 times"));
        assert!(reg.get("broken").is_err());
        assert!(reg.get("ok").is_ok(), "others stay usable");
        assert!(reg.get("testeur").is_ok());
        assert!(reg.diagnostics.len() >= 3);
        assert!(reg
            .diagnostics
            .iter()
            .any(|d| d.source.ends_with("inspecteur.md")));
        assert!(reg.get("nope").unwrap_err().contains("available"));
    }

    #[test]
    fn reload_makes_a_new_snapshot_and_keeps_the_old_one() {
        let d = tempfile::tempdir().unwrap();
        let proj = d.path().join("p");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("x.md"), profile("x", "first")).unwrap();
        let cat = ProfileCatalog::new(ProfileSources {
            project_dir: Some(proj.clone()),
            user_dir: None,
        });
        let running = cat.snapshot().get("x").unwrap();
        std::fs::write(proj.join("x.md"), profile("x", "second")).unwrap();
        assert_eq!(cat.snapshot().get("x").unwrap().description, "first");
        cat.reload();
        assert_eq!(cat.snapshot().get("x").unwrap().description, "second");
        assert_eq!(
            running.description, "first",
            "a started instance keeps its snapshot"
        );
        assert_ne!(
            running.source.revision,
            cat.snapshot().get("x").unwrap().source.revision
        );
    }

    #[test]
    fn search_is_paged() {
        let d = tempfile::tempdir().unwrap();
        let proj = d.path().join("p");
        std::fs::create_dir_all(&proj).unwrap();
        for i in 0..30 {
            std::fs::write(
                proj.join(format!("c{i:02}.md")),
                profile(&format!("custom-{i:02}"), "custom role"),
            )
            .unwrap();
        }
        let reg = ProfileRegistry::load(&ProfileSources {
            project_dir: Some(proj),
            user_dir: None,
        });
        assert_eq!(reg.len(), 37, "no fixed number of custom profiles");
        let (page, total) = reg.search("custom", 1, 10);
        assert_eq!(total, 30);
        assert_eq!(page.len(), 10);
        assert_eq!(page[0].name, "custom-10");
        let (_, total) = reg.search("no-such-role-zz", 0, 10);
        assert_eq!(total, 0);
        let (p, _) = reg.search("MEANINGFUL CHECKS", 0, 10);
        assert_eq!(p[0].name, "testeur");
    }

    #[test]
    fn documented_examples_load_cleanly() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/agents/examples");
        let reg = ProfileRegistry::load(&ProfileSources {
            project_dir: Some(dir),
            user_dir: None,
        });
        assert!(reg.diagnostics.is_empty(), "{:?}", reg.diagnostics);
        assert_eq!(
            reg.get("reviewer").unwrap().source.scope,
            ProfileScope::Project
        );
        let m = reg.get("migration_planner").unwrap();
        assert_eq!(m.max_turns, Some(40));
        assert_eq!(reg.len(), 9);
    }
}
