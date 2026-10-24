//! The skills a person installs in Bricks: copies of standard skill
//! directories in one folder (`<bricks_home>/skills/<name>/`), each a
//! portable Agent Skills directory any compatible client can read.
//!
//! A copy is prepared in a staging folder of the store, validated there with
//! the same parser as every other skill, then published by one rename: an
//! interrupted or refused installation leaves nothing visible, and a name
//! already taken is refused, never overwritten. The source folder is only
//! read; no script is run. Each installation gets an id of its own (kept in
//! a hidden file other clients ignore), so a skill removed and installed
//! again is a new identity: an activation of the old one never comes back.

use crate::document::{
    MAX_DESCRIPTION_CHARS, SkillDocument, name_problems, normalize_name, parse_document,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The hidden file holding an installation's id (not part of the skill).
pub const INSTALL_FILE: &str = ".bricks-install.json";
/// Work folders of the store, never skills.
const STAGING: &str = ".staging";
const TRASH: &str = ".trash";
/// Not copied from an imported folder.
const SKIPPED: &[&str] = &[".git", ".DS_Store", INSTALL_FILE];

static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Changes made to a store by this process (services re-read their
/// sources when it moves).
pub fn generation() -> u64 {
    GENERATION.load(Ordering::SeqCst)
}

fn bump() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
}

/// How an installation came to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallOrigin {
    /// Written in the settings (name, description, instructions).
    Created,
    /// Copied from a folder chosen in the settings.
    Imported,
    /// Installed by the model with the `Skill` tool.
    Model,
}

/// What [`INSTALL_FILE`] holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallRecord {
    pub id: String,
    pub origin: InstallOrigin,
    /// Seconds since the Unix epoch.
    pub installed_at: u64,
}

/// The id of the installation in `dir`, when it has one.
pub fn install_id(dir: &Path) -> Option<String> {
    let bytes = std::fs::read(dir.join(INSTALL_FILE)).ok()?;
    serde_json::from_slice::<InstallRecord>(&bytes)
        .ok()
        .map(|r| r.id)
}

/// The part of a skill an error is about (for a form).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    Name,
    Description,
    Instructions,
    /// The `SKILL.md` as a whole (frontmatter, other fields).
    Document,
    /// The folder (missing `SKILL.md`, unsafe entries, copy).
    Folder,
}

/// Why a store operation was refused or failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StoreError {
    /// The document breaks the standard.
    Invalid {
        field: Field,
        message: String,
    },
    /// Valid, but uses directives Bricks does not implement.
    Incompatible {
        name: String,
        reasons: Vec<String>,
    },
    /// The name belongs to a skill built into Bricks.
    Reserved {
        name: String,
    },
    /// A skill of that name is already installed.
    Exists {
        name: String,
    },
    /// No skill of that name is installed in the store.
    NotInstalled {
        name: String,
    },
    /// An entry of the folder cannot be copied safely.
    Unsafe {
        path: String,
        reason: String,
    },
    Io {
        message: String,
    },
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Invalid { message, .. } => f.write_str(message),
            StoreError::Incompatible { name, reasons } => write!(
                f,
                "skill `{name}` uses directives Bricks does not implement: {}; remove them first",
                reasons.join(", ")
            ),
            StoreError::Reserved { name } => {
                write!(f, "`{name}` is the name of a skill built into Bricks")
            }
            StoreError::Exists { name } => write!(
                f,
                "a skill named `{name}` is already installed; remove it or choose another name"
            ),
            StoreError::NotInstalled { name } => {
                write!(f, "no installed skill is named `{name}`")
            }
            StoreError::Unsafe { path, reason } => write!(f, "`{path}`: {reason}"),
            StoreError::Io { message } => f.write_str(message),
        }
    }
}

impl std::error::Error for StoreError {}

fn io(context: &str, e: std::io::Error) -> StoreError {
    StoreError::Io {
        message: format!("{context}: {e}"),
    }
}

/// A folder checked and ready to install.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checked {
    pub name: String,
    pub description: String,
    /// Files that would be copied, `SKILL.md` included.
    pub files: usize,
    pub bytes: u64,
    pub notes: Vec<String>,
}

/// A published installation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installed {
    pub name: String,
    pub description: String,
    pub id: String,
    pub dir: PathBuf,
    pub files: usize,
    pub notes: Vec<String>,
}

/// One store folder.
#[derive(Debug, Clone)]
pub struct SkillStore {
    dir: PathBuf,
    /// Names no installation may take (the built-in skills).
    reserved: Vec<String>,
}

/// One entry to copy: relative path, and whether it is a folder.
struct Entry {
    rel: PathBuf,
    dir: bool,
    bytes: u64,
}

impl SkillStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            reserved: crate::builtin::BUILTIN
                .iter()
                .map(|b| b.name.to_string())
                .collect(),
        }
    }

    /// The store of a Bricks home folder.
    pub fn in_home(bricks_home: &Path) -> Self {
        Self::new(bricks_home.join("skills"))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn check_name(&self, name: &str) -> Result<(), StoreError> {
        if self.reserved.iter().any(|r| r == name) {
            return Err(StoreError::Reserved { name: name.into() });
        }
        Ok(())
    }

    /// Check `src` as an installation would, without writing anything.
    pub fn validate(&self, src: &Path) -> Result<Checked, StoreError> {
        let entries = plan(src)?;
        let doc = read_document(src, None)?;
        self.check_name(&doc.name)?;
        let mut notes = doc.notes.clone();
        if let Some(dir) = src.file_name().and_then(|n| n.to_str())
            && normalize_name(dir) != doc.name
        {
            notes.push(format!(
                "installed as `{}` (the folder's name differs)",
                doc.name
            ));
        }
        Ok(Checked {
            name: doc.name,
            description: doc.description,
            files: entries.iter().filter(|e| !e.dir).count(),
            bytes: entries.iter().map(|e| e.bytes).sum(),
            notes,
        })
    }

    /// Copy `src` into the store under its skill's name. The copy is
    /// validated again before it is published.
    pub fn install(&self, src: &Path, origin: InstallOrigin) -> Result<Installed, StoreError> {
        let checked = self.validate(src)?;
        let entries = plan(src)?;
        let stage = self.stage(&checked.name)?;
        let result = (|| {
            for e in &entries {
                let to = stage.join(&e.rel);
                if e.dir {
                    std::fs::create_dir_all(&to).map_err(|x| io("cannot prepare the copy", x))?;
                } else {
                    copy_regular(&src.join(&e.rel), &to, &e.rel)?;
                }
            }
            self.publish(&stage, &checked.name, origin)
        })();
        self.unstage(&stage);
        result
    }

    /// Write a new skill from its three parts.
    pub fn create(
        &self,
        name: &str,
        description: &str,
        instructions: &str,
        origin: InstallOrigin,
    ) -> Result<Installed, StoreError> {
        let text = compose(name, description, instructions)?;
        let name = normalize_name(name);
        self.check_name(&name)?;
        let stage = self.stage(&name)?;
        let result = std::fs::write(stage.join("SKILL.md"), text)
            .map_err(|e| io("cannot write SKILL.md", e))
            .and_then(|_| self.publish(&stage, &name, origin));
        self.unstage(&stage);
        result
    }

    /// Remove the installed skill `name` (only a copy in this store).
    pub fn remove(&self, name: &str) -> Result<String, StoreError> {
        let name = normalize_name(name);
        self.check_name(&name)?;
        if !name_problems(&name, None).is_empty() {
            return Err(StoreError::NotInstalled { name });
        }
        let dir = self.dir.join(&name);
        let meta = std::fs::symlink_metadata(&dir)
            .map_err(|_| StoreError::NotInstalled { name: name.clone() })?;
        if !meta.is_dir() || !dir.join("SKILL.md").is_file() {
            return Err(StoreError::NotInstalled { name });
        }
        let trash = self.dir.join(TRASH);
        std::fs::create_dir_all(&trash).map_err(|e| io("cannot remove the skill", e))?;
        let gone = trash.join(uuid::Uuid::new_v4().simple().to_string());
        // One rename takes it out of the store; the rest is cleanup.
        std::fs::rename(&dir, &gone).map_err(|e| match e.kind() {
            // Removed meanwhile by someone else.
            std::io::ErrorKind::NotFound => StoreError::NotInstalled { name: name.clone() },
            _ => io("cannot remove the skill", e),
        })?;
        bump();
        let _ = std::fs::remove_dir_all(&gone);
        Ok(name)
    }

    /// A fresh staging folder named `name`, inside the store (same file
    /// system as the final folder, so publishing is one rename).
    fn stage(&self, name: &str) -> Result<PathBuf, StoreError> {
        let parent = self
            .dir
            .join(STAGING)
            .join(uuid::Uuid::new_v4().simple().to_string());
        let stage = parent.join(name);
        std::fs::create_dir_all(&stage).map_err(|e| io("cannot prepare the installation", e))?;
        Ok(stage)
    }

    fn unstage(&self, stage: &Path) {
        if let Some(parent) = stage.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    /// Validate the prepared copy, mark it, and move it into place unless
    /// the name is taken.
    fn publish(
        &self,
        stage: &Path,
        name: &str,
        origin: InstallOrigin,
    ) -> Result<Installed, StoreError> {
        let doc = read_document(stage, Some(name))?;
        let record = InstallRecord {
            id: uuid::Uuid::new_v4().simple().to_string()[..12].to_string(),
            origin,
            installed_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        };
        let json = serde_json::to_vec_pretty(&record).map_err(|e| StoreError::Io {
            message: e.to_string(),
        })?;
        std::fs::write(stage.join(INSTALL_FILE), json)
            .map_err(|e| io("cannot mark the installation", e))?;
        let dest = self.dir.join(name);
        // Taking the name is atomic: a second installer (or one that got
        // there first) fails here. The rename then replaces only this
        // empty folder; anything else in it makes it fail.
        match std::fs::create_dir(&dest) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(StoreError::Exists { name: name.into() });
            }
            Err(e) => return Err(io("cannot install the skill", e)),
        }
        if let Err(e) = std::fs::rename(stage, &dest) {
            let _ = std::fs::remove_dir(&dest);
            return Err(io("cannot install the skill", e));
        }
        bump();
        let files = count_files(&dest);
        Ok(Installed {
            name: doc.name,
            description: doc.description,
            id: record.id,
            dir: dest,
            files,
            notes: doc.notes,
        })
    }
}

/// The `SKILL.md` of a new skill, or the field to correct.
pub fn compose(name: &str, description: &str, instructions: &str) -> Result<String, StoreError> {
    let problems = name_problems(name, None);
    if !problems.is_empty() {
        return Err(StoreError::Invalid {
            field: Field::Name,
            message: problems.join("; "),
        });
    }
    let description = description.trim();
    if description.is_empty() {
        return Err(StoreError::Invalid {
            field: Field::Description,
            message: "`description` is empty: say what the skill does and when to use it".into(),
        });
    }
    if description.chars().count() > MAX_DESCRIPTION_CHARS {
        return Err(StoreError::Invalid {
            field: Field::Description,
            message: format!(
                "`description` has {} characters (at most {MAX_DESCRIPTION_CHARS})",
                description.chars().count()
            ),
        });
    }
    let instructions = instructions.trim();
    if instructions.is_empty() {
        return Err(StoreError::Invalid {
            field: Field::Instructions,
            message: "the instructions are empty".into(),
        });
    }
    // A JSON string is a valid YAML double-quoted scalar: any text, quoted
    // and escaped.
    let quoted = serde_json::to_string(description).unwrap_or_default();
    let text = format!(
        "---\nname: {}\ndescription: {quoted}\n---\n\n{instructions}\n",
        normalize_name(name)
    );
    // The same parser as every skill: what is written is what will be read.
    parse_document(&text, Some(&normalize_name(name))).map_err(|e| StoreError::Invalid {
        field: Field::Document,
        message: e.0,
    })?;
    Ok(text)
}

/// Read and validate `dir/SKILL.md`; `name` is the directory's final name
/// (`None`: any).
fn read_document(dir: &Path, name: Option<&str>) -> Result<SkillDocument, StoreError> {
    let file = dir.join("SKILL.md");
    match std::fs::symlink_metadata(&file) {
        Ok(m) if m.is_file() => {}
        Ok(_) => {
            return Err(StoreError::Unsafe {
                path: "SKILL.md".into(),
                reason: "not a regular file".into(),
            });
        }
        Err(_) => {
            return Err(StoreError::Invalid {
                field: Field::Folder,
                message: "the folder has no SKILL.md".into(),
            });
        }
    }
    let bytes = crate::registry::read_bounded(&file).map_err(|e| StoreError::Io {
        message: format!("SKILL.md cannot be read: {e}"),
    })?;
    let text = String::from_utf8(bytes).map_err(|_| StoreError::Invalid {
        field: Field::Document,
        message: "SKILL.md is not UTF-8 text".into(),
    })?;
    let doc = parse_document(&text, name).map_err(|e| StoreError::Invalid {
        field: field_of(&e.0),
        message: e.0,
    })?;
    if !doc.is_compatible() {
        return Err(StoreError::Incompatible {
            name: doc.name.clone(),
            reasons: doc.unsupported.clone(),
        });
    }
    Ok(doc)
}

/// The field a parser message is about (it names it first).
fn field_of(message: &str) -> Field {
    if message.starts_with("`name`") {
        Field::Name
    } else if message.starts_with("`description`") {
        Field::Description
    } else {
        Field::Document
    }
}

/// Everything under `src` to copy, checked: no symbolic link (it could
/// lead outside the folder), no special file. Nothing is read but the
/// listing.
fn plan(src: &Path) -> Result<Vec<Entry>, StoreError> {
    let meta = std::fs::symlink_metadata(src).map_err(|e| StoreError::Invalid {
        field: Field::Folder,
        message: format!("`{}` cannot be read: {e}", src.display()),
    })?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(StoreError::Invalid {
            field: Field::Folder,
            message: format!("`{}` is not a folder", src.display()),
        });
    }
    let mut out = Vec::new();
    walk(src, Path::new(""), &mut out)?;
    Ok(out)
}

fn walk(base: &Path, rel: &Path, out: &mut Vec<Entry>) -> Result<(), StoreError> {
    let dir = base.join(rel);
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .map_err(|e| io(&format!("`{}` cannot be listed", dir.display()), e))?
        .collect::<Result<_, _>>()
        .map_err(|e| io(&format!("`{}` cannot be listed", dir.display()), e))?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name();
        if SKIPPED.iter().any(|s| name == *s) {
            continue;
        }
        let rel = rel.join(&name);
        let shown = rel.to_string_lossy().replace('\\', "/");
        let meta = std::fs::symlink_metadata(entry.path()).map_err(|e| StoreError::Unsafe {
            path: shown.clone(),
            reason: e.to_string(),
        })?;
        let kind = meta.file_type();
        if kind.is_symlink() {
            return Err(StoreError::Unsafe {
                path: shown,
                reason: "symbolic links are not imported (one could lead outside the folder)"
                    .into(),
            });
        }
        if kind.is_dir() {
            out.push(Entry {
                rel: rel.clone(),
                dir: true,
                bytes: 0,
            });
            walk(base, &rel, out)?;
        } else if kind.is_file() {
            out.push(Entry {
                rel,
                dir: false,
                bytes: meta.len(),
            });
        } else {
            return Err(StoreError::Unsafe {
                path: shown,
                reason: "not a regular file".into(),
            });
        }
    }
    Ok(())
}

/// Copy one regular file. It is opened without following a link and checked
/// once open: a file swapped for a link or a FIFO since the plan is refused,
/// never read through.
fn copy_regular(from: &Path, to: &Path, rel: &Path) -> Result<(), StoreError> {
    let shown = rel.to_string_lossy().replace('\\', "/");
    let changed = |reason: String| StoreError::Unsafe {
        path: shown.clone(),
        reason,
    };
    let mut input = open_no_follow(from).map_err(|e| {
        #[cfg(unix)]
        if e.raw_os_error() == Some(libc::ELOOP) {
            return changed("became a symbolic link during the copy".into());
        }
        changed(format!("cannot be read: {e}"))
    })?;
    let meta = input.metadata().map_err(|e| changed(e.to_string()))?;
    if !meta.is_file() {
        return Err(changed("changed during the copy".into()));
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io("cannot prepare the copy", e))?;
    }
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to)
        .map_err(|e| io(&format!("`{shown}` cannot be copied"), e))?;
    std::io::copy(&mut input, &mut output)
        .map_err(|e| io(&format!("`{shown}` cannot be copied"), e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Modes are kept (a script stays executable); nothing is run.
        let mode = meta.permissions().mode() & 0o777;
        let _ = std::fs::set_permissions(to, std::fs::Permissions::from_mode(mode));
    }
    Ok(())
}

#[cfg(unix)]
fn open_no_follow(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
}

#[cfg(not(unix))]
fn open_no_follow(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::File::open(path)
}

fn count_files(dir: &Path) -> usize {
    let mut n = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            if e.file_name() == INSTALL_FILE {
                continue;
            }
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push(e.path()),
                Ok(t) if t.is_file() => n += 1,
                _ => {}
            }
        }
    }
    n
}
