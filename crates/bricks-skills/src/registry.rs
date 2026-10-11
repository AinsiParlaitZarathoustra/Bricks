//! Sources of skills, their discovery and the resolution of a name.
//!
//! Default sources, by priority (the first wins a name):
//!
//! 1. `<project>/.agents/skills/`
//! 2. `<project>/.claude/skills/`
//! 3. `~/.agents/skills/`
//! 4. `~/.claude/skills/`
//! 5. the skills built into Bricks.
//!
//! Each source holds `<name>/SKILL.md` directories, one level deep. A
//! missing folder is normal; an unreadable or invalid `SKILL.md` is a
//! diagnostic. Symbolic links are not followed (reported). The scan is
//! bounded ([`Limits`]); only metadata is kept, never the instructions.
//!
//! Resolution only looks at the sources the [`Trust`] allows: a project
//! source not yet approved cannot provide a skill — nor block a personal or
//! built-in one. Among allowed sources the candidate of highest priority
//! wins; if it is invalid or incompatible, the name fails: there is no
//! silent fallback to a lower homonym.

use crate::document::{normalize_name, parse_document, SkillDocument, MAX_SKILL_BYTES};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Who a source belongs to; decides how it is trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// In the project's folder: usable once the user approves it.
    Project,
    /// In the user's home folder.
    Personal,
    /// Compiled into Bricks.
    Builtin,
    /// Given explicitly by an embedder (SDK), trusted as it says.
    Explicit,
}

/// One place skills are read from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillRoot {
    /// Stable identity (`project:.agents`, `user:.claude`, `builtin`, …).
    pub id: String,
    pub scope: Scope,
    /// The folder (`None` for the built-in source).
    pub path: Option<PathBuf>,
    /// For `Explicit` roots only: whether the embedder trusts it.
    #[serde(default)]
    pub trusted: bool,
}

impl SkillRoot {
    pub fn explicit(id: impl Into<String>, path: impl Into<PathBuf>, trusted: bool) -> Self {
        Self {
            id: format!("sdk:{}", id.into()),
            scope: Scope::Explicit,
            path: Some(path.into()),
            trusted,
        }
    }

    /// How a person reads it.
    pub fn label(&self) -> String {
        match &self.path {
            Some(p) => format!("{} ({})", self.id, p.display()),
            None => self.id.clone(),
        }
    }
}

/// The five default sources of `project`, with `home` (injected, so tests
/// never read the real one).
pub fn standard_roots(project: &Path, home: Option<&Path>) -> Vec<SkillRoot> {
    let mut roots = vec![
        SkillRoot {
            id: "project:.agents".into(),
            scope: Scope::Project,
            path: Some(project.join(".agents").join("skills")),
            trusted: false,
        },
        SkillRoot {
            id: "project:.claude".into(),
            scope: Scope::Project,
            path: Some(project.join(".claude").join("skills")),
            trusted: false,
        },
    ];
    if let Some(home) = home {
        roots.push(SkillRoot {
            id: "user:.agents".into(),
            scope: Scope::Personal,
            path: Some(home.join(".agents").join("skills")),
            trusted: false,
        });
        roots.push(SkillRoot {
            id: "user:.claude".into(),
            scope: Scope::Personal,
            path: Some(home.join(".claude").join("skills")),
            trusted: false,
        });
    }
    roots.push(SkillRoot {
        id: "builtin".into(),
        scope: Scope::Builtin,
        path: None,
        trusted: true,
    });
    roots
}

/// Bounds of a discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Entries looked at in one source folder.
    pub max_entries_per_root: usize,
    /// Candidates kept in total.
    pub max_candidates: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_entries_per_root: 512,
            max_candidates: 1_024,
        }
    }
}

/// The project decision, per session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectDecision {
    /// Not asked or not answered yet: project skills are not usable.
    #[default]
    Pending,
    Granted,
    Denied,
}

/// What may be used. The project decision is made by a person (or an
/// embedder's explicit call), never by a file or a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trust {
    pub project: ProjectDecision,
    /// Personal skills (home folder): usable unless the embedder says not.
    pub personal: bool,
}

impl Default for Trust {
    fn default() -> Self {
        Self {
            project: ProjectDecision::Pending,
            personal: true,
        }
    }
}

impl Trust {
    pub fn allows(&self, root: &SkillRoot) -> bool {
        match root.scope {
            Scope::Project => self.project == ProjectDecision::Granted,
            Scope::Personal => self.personal,
            Scope::Builtin => true,
            Scope::Explicit => root.trusted,
        }
    }
}

/// The state of one candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CandidateState {
    Valid,
    /// Valid, but uses directives Bricks does not implement.
    Incompatible {
        reasons: Vec<String>,
    },
    Invalid {
        error: String,
    },
}

/// One `SKILL.md` found in a source (metadata only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    /// Index of its source in [`Registry::roots`].
    pub root: usize,
    pub source: String,
    pub scope: Scope,
    /// The name this candidate answers to: its `name`, or the
    /// (normalised) name of its directory when the document gives none.
    pub key: String,
    /// Directory name, or built-in name.
    pub identity: String,
    pub description: Option<String>,
    /// The skill's directory (`None` when built in).
    pub dir: Option<PathBuf>,
    pub file: Option<PathBuf>,
    /// SHA-256 (16 hex digits) of the file as discovered.
    pub revision: Option<String>,
    pub bytes: usize,
    pub state: CandidateState,
    pub notes: Vec<String>,
}

/// A diagnostic of the discovery (not tied to one valid candidate).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub source: String,
    pub path: Option<PathBuf>,
    pub message: String,
}

/// A skill compiled into Bricks.
#[derive(Debug, Clone, Copy)]
pub struct BuiltinSkill {
    pub name: &'static str,
    pub text: &'static str,
}

/// One generation of the registry: what the sources held when read.
#[derive(Debug, Clone, Default)]
pub struct Registry {
    pub generation: u64,
    pub roots: Vec<SkillRoot>,
    /// Sorted by key, then priority (source order), then identity.
    pub candidates: Vec<Candidate>,
    pub diagnostics: Vec<Diagnostic>,
    /// The scan stopped at a limit somewhere.
    pub truncated: bool,
    builtins: Vec<BuiltinSkill>,
}

/// Why a name cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResolveError {
    NotFound {
        name: String,
    },
    /// Only in a project source that is not approved (pending or denied).
    NotTrusted {
        name: String,
        decision: ProjectDecision,
    },
    Invalid {
        name: String,
        source: String,
        error: String,
    },
    Incompatible {
        name: String,
        source: String,
        reasons: Vec<String>,
    },
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::NotFound { name } => write!(f, "no skill named `{name}`"),
            ResolveError::NotTrusted { name, decision } => write!(
                f,
                "skill `{name}` is only in this project's skills, which are {} for this session",
                match decision {
                    ProjectDecision::Denied => "refused",
                    _ => "not approved",
                }
            ),
            ResolveError::Invalid {
                name,
                source,
                error,
            } => write!(f, "skill `{name}` ({source}) is invalid: {error}"),
            ResolveError::Incompatible {
                name,
                source,
                reasons,
            } => write!(
                f,
                "skill `{name}` ({source}) is incompatible with Bricks: it uses {}; convert it \
                 before activating it",
                reasons.join(", ")
            ),
        }
    }
}

impl std::error::Error for ResolveError {}

/// SHA-256 of a file, as a revision (16 hex digits).
pub fn revision_of(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Read at most `MAX_SKILL_BYTES + 1` bytes: a larger file is refused
/// without reading it whole.
pub(crate) fn read_bounded(path: &Path) -> Result<Vec<u8>, String> {
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    f.take(MAX_SKILL_BYTES as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|e| e.to_string())?;
    Ok(buf)
}

fn candidate_of(
    root_index: usize,
    root: &SkillRoot,
    identity: String,
    dir: Option<PathBuf>,
    file: Option<PathBuf>,
    bytes: Result<Vec<u8>, String>,
) -> Candidate {
    let dir_key = normalize_name(&identity);
    let mut c = Candidate {
        root: root_index,
        source: root.id.clone(),
        scope: root.scope,
        key: dir_key,
        identity: identity.clone(),
        description: None,
        dir: dir.clone(),
        file,
        revision: None,
        bytes: 0,
        state: CandidateState::Valid,
        notes: Vec::new(),
    };
    let bytes = match bytes {
        Ok(b) => b,
        Err(e) => {
            c.state = CandidateState::Invalid {
                error: format!("cannot be read: {e}"),
            };
            return c;
        }
    };
    c.bytes = bytes.len();
    c.revision = Some(revision_of(&bytes));
    let text = match String::from_utf8(bytes) {
        Ok(t) => t,
        Err(_) => {
            c.state = CandidateState::Invalid {
                error: "not UTF-8 text".into(),
            };
            return c;
        }
    };
    let dir_name = dir.as_ref().map(|_| identity.as_str());
    match parse_document(&text, dir_name) {
        Ok(d) => apply(&mut c, &d),
        Err(e) => c.state = CandidateState::Invalid { error: e.0 },
    }
    c
}

fn apply(c: &mut Candidate, d: &SkillDocument) {
    c.key = d.name.clone();
    c.description = Some(d.description.clone());
    c.notes = d.notes.clone();
    for issue in crate::security::scan_text(&d.body) {
        c.notes.push(format!("review: {issue}"));
    }
    if !d.is_compatible() {
        c.state = CandidateState::Incompatible {
            reasons: d.unsupported.clone(),
        };
    }
}

impl Registry {
    /// Read the sources. Nothing is executed or written.
    pub fn discover(
        generation: u64,
        roots: Vec<SkillRoot>,
        builtins: &[BuiltinSkill],
        limits: Limits,
    ) -> Self {
        let mut reg = Registry {
            generation,
            roots: roots.clone(),
            builtins: builtins.to_vec(),
            ..Default::default()
        };
        for (i, root) in roots.iter().enumerate() {
            if reg.candidates.len() >= limits.max_candidates {
                reg.truncated = true;
                break;
            }
            match &root.path {
                None => {
                    for b in builtins {
                        reg.candidates.push(candidate_of(
                            i,
                            root,
                            b.name.to_string(),
                            None,
                            None,
                            Ok(b.text.as_bytes().to_vec()),
                        ));
                    }
                }
                Some(path) => reg.scan(i, root, path, limits),
            }
        }
        reg.candidates.sort_by(|a, b| {
            a.key
                .cmp(&b.key)
                .then(a.root.cmp(&b.root))
                .then(a.identity.cmp(&b.identity))
        });
        reg
    }

    fn scan(&mut self, i: usize, root: &SkillRoot, path: &Path, limits: Limits) {
        let entries = match std::fs::read_dir(path) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) => {
                self.diagnostics.push(Diagnostic {
                    source: root.id.clone(),
                    path: Some(path.to_path_buf()),
                    message: format!("cannot be listed: {e}"),
                });
                return;
            }
        };
        // Sorted, so the file system's order never changes the result.
        let mut names: Vec<(String, PathBuf)> = Vec::new();
        for (n, entry) in entries.enumerate() {
            if n >= limits.max_entries_per_root {
                self.truncated = true;
                self.diagnostics.push(Diagnostic {
                    source: root.id.clone(),
                    path: Some(path.to_path_buf()),
                    message: format!(
                        "more than {} entries: the rest is not read",
                        limits.max_entries_per_root
                    ),
                });
                break;
            }
            let Ok(entry) = entry else { continue };
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            names.push((name, entry.path()));
        }
        names.sort();
        for (name, entry_path) in names {
            if self.candidates.len() >= limits.max_candidates {
                self.truncated = true;
                return;
            }
            let meta = match std::fs::symlink_metadata(&entry_path) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.file_type().is_symlink() {
                self.diagnostics.push(Diagnostic {
                    source: root.id.clone(),
                    path: Some(entry_path.clone()),
                    message: "symbolic link not followed".into(),
                });
                continue;
            }
            if !meta.is_dir() {
                continue;
            }
            let file = entry_path.join("SKILL.md");
            match std::fs::symlink_metadata(&file) {
                Ok(m) if m.file_type().is_symlink() => {
                    self.diagnostics.push(Diagnostic {
                        source: root.id.clone(),
                        path: Some(file.clone()),
                        message: "SKILL.md is a symbolic link: not followed".into(),
                    });
                    continue;
                }
                Ok(m) if m.is_file() => {}
                _ => continue, // a folder without SKILL.md is not a skill
            }
            let bytes = read_bounded(&file);
            self.candidates.push(candidate_of(
                i,
                root,
                name,
                Some(entry_path),
                Some(file),
                bytes,
            ));
        }
    }

    pub fn root(&self, c: &Candidate) -> &SkillRoot {
        &self.roots[c.root]
    }

    fn group(&self, key: &str) -> impl Iterator<Item = &Candidate> {
        let key = normalize_name(key);
        self.candidates.iter().filter(move |c| c.key == key)
    }

    /// The candidate a name resolves to under `trust`, without loading it.
    pub fn resolve(&self, name: &str, trust: &Trust) -> Result<&Candidate, ResolveError> {
        let key = normalize_name(name);
        let mut hidden = false;
        for c in self.group(&key) {
            if !trust.allows(self.root(c)) {
                hidden = true;
                continue;
            }
            return match &c.state {
                CandidateState::Valid => Ok(c),
                CandidateState::Invalid { error } => Err(ResolveError::Invalid {
                    name: key,
                    source: c.source.clone(),
                    error: error.clone(),
                }),
                CandidateState::Incompatible { reasons } => Err(ResolveError::Incompatible {
                    name: key,
                    source: c.source.clone(),
                    reasons: reasons.clone(),
                }),
            };
        }
        if hidden {
            Err(ResolveError::NotTrusted {
                name: key,
                decision: trust.project,
            })
        } else {
            Err(ResolveError::NotFound { name: key })
        }
    }

    /// The skills the model may activate under `trust`: the winner of each
    /// name, when valid and compatible. Sorted by name.
    pub fn activable(&self, trust: &Trust) -> Vec<&Candidate> {
        let mut out = Vec::new();
        let mut last: Option<&str> = None;
        for c in &self.candidates {
            if last == Some(c.key.as_str()) || !trust.allows(self.root(c)) {
                continue;
            }
            last = Some(c.key.as_str());
            if c.state == CandidateState::Valid {
                out.push(c);
            }
        }
        out
    }

    /// The text of a candidate, read now (a built-in one from the binary).
    pub fn read(&self, c: &Candidate) -> Result<(String, String), String> {
        match &c.file {
            None => {
                let b = self
                    .builtins
                    .iter()
                    .find(|b| b.name == c.identity)
                    .ok_or("built-in skill missing")?;
                Ok((b.text.to_string(), revision_of(b.text.as_bytes())))
            }
            Some(file) => {
                let meta = std::fs::symlink_metadata(file).map_err(|e| e.to_string())?;
                if meta.file_type().is_symlink() {
                    return Err("SKILL.md became a symbolic link".into());
                }
                let bytes = read_bounded(file)?;
                let rev = revision_of(&bytes);
                let text = String::from_utf8(bytes).map_err(|_| "not UTF-8 text".to_string())?;
                Ok((text, rev))
            }
        }
    }

    /// Project candidates (for a person to inspect before approving).
    pub fn project_candidates(&self) -> Vec<&Candidate> {
        self.candidates
            .iter()
            .filter(|c| c.scope == Scope::Project)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn write(root: &Path, dir: &str, front: &str) {
        let d = root.join(dir);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("SKILL.md"),
            format!("---\n{front}---\nbody of {dir}\n"),
        )
        .unwrap();
    }

    fn ok(dir: &str, desc: &str) -> String {
        format!("name: {dir}\ndescription: {desc}\n")
    }

    struct Fixture {
        _t: tempfile::TempDir,
        project: PathBuf,
        home: PathBuf,
    }

    fn fixture() -> Fixture {
        let t = tempfile::tempdir().unwrap();
        let project = t.path().join("p");
        let home = t.path().join("h");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        Fixture {
            _t: t,
            project,
            home,
        }
    }

    const BUILTIN: &[BuiltinSkill] = &[BuiltinSkill {
        name: "shared",
        text: "---\nname: shared\ndescription: built in\n---\nbuiltin body\n",
    }];

    fn reg(f: &Fixture) -> Registry {
        Registry::discover(
            1,
            standard_roots(&f.project, Some(&f.home)),
            BUILTIN,
            Limits::default(),
        )
    }

    const GRANTED: Trust = Trust {
        project: ProjectDecision::Granted,
        personal: true,
    };

    #[test]
    fn five_sources_in_priority_order() {
        let f = fixture();
        write(
            &f.project.join(".agents/skills"),
            "shared",
            &ok("shared", "p-agents"),
        );
        write(
            &f.project.join(".claude/skills"),
            "shared",
            &ok("shared", "p-claude"),
        );
        write(
            &f.home.join(".agents/skills"),
            "shared",
            &ok("shared", "h-agents"),
        );
        write(
            &f.home.join(".claude/skills"),
            "shared",
            &ok("shared", "h-claude"),
        );
        let r = reg(&f);
        let order: Vec<&str> = r.group("shared").map(|c| c.source.as_str()).collect();
        assert_eq!(
            order,
            [
                "project:.agents",
                "project:.claude",
                "user:.agents",
                "user:.claude",
                "builtin"
            ]
        );
        let win = |t: &Trust| r.resolve("shared", t).unwrap().description.clone().unwrap();
        assert_eq!(win(&GRANTED), "p-agents");
        // Without approval, the project cannot win, nor block the others.
        assert_eq!(win(&Trust::default()), "h-agents");
        let no_personal = Trust {
            project: ProjectDecision::Denied,
            personal: false,
        };
        assert_eq!(win(&no_personal), "built in");
        // One catalog entry per name.
        assert_eq!(r.activable(&GRANTED).len(), 1);
    }

    #[test]
    fn missing_sources_are_normal() {
        let f = fixture();
        let r = reg(&f);
        assert!(r.diagnostics.is_empty());
        assert_eq!(r.activable(&GRANTED).len(), 1);
        assert!(matches!(
            r.resolve("absent", &GRANTED),
            Err(ResolveError::NotFound { .. })
        ));
    }

    #[test]
    fn an_invalid_winner_does_not_fall_back() {
        let f = fixture();
        write(
            &f.project.join(".agents/skills"),
            "shared",
            "name: shared\n",
        );
        let r = reg(&f);
        assert!(matches!(
            r.resolve("shared", &GRANTED),
            Err(ResolveError::Invalid { .. })
        ));
        assert!(r.activable(&GRANTED).is_empty());
        // A broken frontmatter is keyed by its directory, so the conflict
        // is still seen.
        let f = fixture();
        write(&f.home.join(".agents/skills"), "shared", "name: [\n");
        let r = reg(&f);
        assert!(matches!(
            r.resolve("shared", &GRANTED),
            Err(ResolveError::Invalid { .. })
        ));
    }

    #[test]
    fn untrusted_project_skills_are_neither_usable_nor_blocking() {
        let f = fixture();
        write(
            &f.project.join(".claude/skills"),
            "only-here",
            &ok("only-here", "d"),
        );
        write(
            &f.project.join(".claude/skills"),
            "broken",
            "name: broken\n",
        );
        write(
            &f.home.join(".agents/skills"),
            "broken",
            &ok("broken", "personal"),
        );
        let r = reg(&f);
        let pending = Trust::default();
        assert!(matches!(
            r.resolve("only-here", &pending),
            Err(ResolveError::NotTrusted { .. })
        ));
        assert_eq!(
            r.resolve("broken", &pending).unwrap().source,
            "user:.agents"
        );
        let names: Vec<&str> = r
            .activable(&pending)
            .iter()
            .map(|c| c.key.as_str())
            .collect();
        assert_eq!(names, ["broken", "shared"]);
        assert_eq!(r.project_candidates().len(), 2);
    }

    #[test]
    fn incompatible_documents_are_excluded() {
        let f = fixture();
        write(
            &f.home.join(".agents/skills"),
            "fork",
            "name: fork\ndescription: d\ncontext: fork\n",
        );
        let r = reg(&f);
        assert!(matches!(
            r.resolve("fork", &GRANTED),
            Err(ResolveError::Incompatible { .. })
        ));
        assert!(r.activable(&GRANTED).iter().all(|c| c.key != "fork"));
    }

    #[test]
    fn historical_locations_are_not_read() {
        let f = fixture();
        write(&f.project.join(".cersei/skills"), "old", &ok("old", "d"));
        std::fs::create_dir_all(f.project.join(".claude/commands")).unwrap();
        std::fs::write(f.project.join(".claude/commands/cmd.md"), "Do $ARGUMENTS").unwrap();
        write(&f.project.join(".bricks/skills"), "new", &ok("new", "d"));
        let r = reg(&f);
        for n in ["old", "cmd", "new"] {
            assert!(r.resolve(n, &GRANTED).is_err(), "{n}");
        }
    }

    #[test]
    fn explicit_roots_are_trusted_only_as_declared() {
        let t = tempfile::tempdir().unwrap();
        write(t.path(), "sdk-one", &ok("sdk-one", "d"));
        let roots = vec![
            SkillRoot::explicit("a", t.path(), false),
            SkillRoot::explicit("b", t.path(), true),
        ];
        let r = Registry::discover(1, roots, &[], Limits::default());
        let c = r.resolve("sdk-one", &Trust::default()).unwrap();
        assert_eq!(c.source, "sdk:b");
    }

    #[test]
    fn the_result_does_not_depend_on_creation_order() {
        let mk = |order: &[&str]| {
            let f = fixture();
            for d in order {
                write(&f.home.join(".agents/skills"), d, &ok(d, d));
            }
            let r = reg(&f);
            let keys: Vec<String> = r.candidates.iter().map(|c| c.key.clone()).collect();
            (keys, f)
        };
        let (a, _f1) = mk(&["zeta", "alpha", "mid"]);
        let (b, _f2) = mk(&["mid", "zeta", "alpha"]);
        assert_eq!(a, b);
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_links_are_not_followed() {
        let f = fixture();
        let outside = f.home.join("outside");
        write(&outside, "linked", &ok("linked", "d"));
        let src = f.home.join(".agents/skills");
        std::fs::create_dir_all(&src).unwrap();
        std::os::unix::fs::symlink(outside.join("linked"), src.join("linked")).unwrap();
        let r = reg(&f);
        assert!(r.resolve("linked", &GRANTED).is_err());
        assert!(r.diagnostics.iter().any(|d| d.message.contains("symbolic")));
    }

    #[test]
    fn the_scan_is_bounded() {
        let f = fixture();
        for i in 0..20 {
            let n = format!("s{i:02}");
            write(&f.home.join(".agents/skills"), &n, &ok(&n, "d"));
        }
        let r = Registry::discover(
            1,
            standard_roots(&f.project, Some(&f.home)),
            BUILTIN,
            Limits {
                max_entries_per_root: 5,
                max_candidates: 100,
            },
        );
        assert!(r.truncated);
        assert!(
            r.candidates
                .iter()
                .filter(|c| c.scope == Scope::Personal)
                .count()
                <= 5
        );
    }

    #[test]
    fn oversized_or_binary_files_are_diagnosed() {
        let f = fixture();
        let d = f.home.join(".agents/skills/big");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("SKILL.md"), vec![b'a'; MAX_SKILL_BYTES + 10]).unwrap();
        let d2 = f.home.join(".agents/skills/bin");
        std::fs::create_dir_all(&d2).unwrap();
        std::fs::write(d2.join("SKILL.md"), [0xff, 0xfe, 0x00]).unwrap();
        let r = reg(&f);
        assert!(matches!(
            r.resolve("big", &GRANTED),
            Err(ResolveError::Invalid { .. })
        ));
        assert!(matches!(
            r.resolve("bin", &GRANTED),
            Err(ResolveError::Invalid { .. })
        ));
    }
}
