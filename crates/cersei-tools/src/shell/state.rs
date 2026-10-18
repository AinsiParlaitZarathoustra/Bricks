//! The state the commands of one session share: working directory, exported
//! variables, functions and aliases.
//!
//! Every ordinary command is its own process. It starts from a copy of this
//! state (its *launch snapshot*), and when it ends normally its shell records
//! its final state in a file. The changes it made, compared with its own
//! snapshot, are published here:
//!
//! * a field the command did not change is never written;
//! * for one field, the command launched most recently among those that
//!   publish wins, whatever the order of their ends: an older command that
//!   ends late never restores a previous directory or value;
//! * `PWD` and `OLDPWD` move together.
//!
//! Not kept: non-exported shell variables, shell options, and what a command
//! changes after it was interrupted, timed out, or replaced its shell with
//! `exec`. Functions and aliases are restored by a prelude sourced before the
//! command.

use parking_lot::Mutex;
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Variables the shell manages itself: never taken from the process
/// environment, never recorded as the session's.
const MANAGED: &[&str] = &[
    "PWD",
    "OLDPWD",
    "SHLVL",
    "_",
    "BASH_ENV",
    "ENV",
    "PROMPT_COMMAND",
    "SHELLOPTS",
    "BASHOPTS",
];

/// Working directory: `pwd` is the logical one (`PWD`), `oldpwd` is `OLDPWD`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cwd {
    pub pwd: PathBuf,
    pub oldpwd: Option<PathBuf>,
}

/// The state at one moment: what a command starts from, or ends with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub cwd: Cwd,
    pub exports: BTreeMap<String, OsString>,
    /// Function definitions, as bash prints them (`declare -f`).
    pub functions: BTreeMap<String, Vec<u8>>,
    /// Alias definitions, as bash prints them (`alias name`).
    pub aliases: BTreeMap<String, Vec<u8>>,
}

#[derive(Debug, Clone, Default)]
struct Field<T> {
    value: T,
    /// Launch sequence of the command that last wrote the field (0: initial).
    seq: u64,
}

#[derive(Debug, Default)]
struct Inner {
    /// Last launch sequence handed out.
    next_seq: u64,
    cwd: Field<Cwd>,
    /// `None`: unset by a command.
    exports: BTreeMap<String, Field<Option<OsString>>>,
    functions: BTreeMap<String, Field<Option<Vec<u8>>>>,
    aliases: BTreeMap<String, Field<Option<Vec<u8>>>>,
}

/// The session's state. Changes are counted in [`ShellState::generation`].
#[derive(Debug)]
pub struct ShellState {
    inner: Mutex<Inner>,
    generation: AtomicU64,
}

/// What a command is started with.
pub struct Launch {
    /// Launch sequence: orders the commands for conflicts.
    pub seq: u64,
    /// The state the command starts from (`cwd` is where it really starts).
    pub base: Snapshot,
    /// The exported variables, exactly the command's environment.
    pub env: Vec<(OsString, OsString)>,
    /// Script sourced before the command: options, `OLDPWD`, aliases, functions.
    pub prelude: Vec<u8>,
    /// Facts to tell the caller (the working directory no longer exists).
    pub notes: Vec<String>,
}

impl Default for ShellState {
    fn default() -> Self {
        Self::from_env()
    }
}

impl ShellState {
    /// A state whose exported variables are the process environment.
    pub fn from_env() -> Self {
        let mut exports = BTreeMap::new();
        for (name, value) in std::env::vars_os() {
            let Some(name) = name.to_str().map(str::to_string) else {
                continue;
            };
            if MANAGED.contains(&name.as_str())
                || name.starts_with("BASH_FUNC_")
                || !valid_name(&name)
            {
                continue;
            }
            exports.insert(
                name,
                Field {
                    value: Some(value),
                    seq: 0,
                },
            );
        }
        Self {
            inner: Mutex::new(Inner {
                exports,
                ..Inner::default()
            }),
            generation: AtomicU64::new(0),
        }
    }

    /// Counts the changes published so far (approvals compare it).
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// Hands out a launch sequence and the state a command starts from.
    /// `default_cwd` is used when no directory was ever set, or the recorded
    /// one no longer exists.
    pub fn launch(&self, default_cwd: &Path) -> Launch {
        let mut inner = self.inner.lock();
        inner.next_seq += 1;
        let seq = inner.next_seq;
        if inner.cwd.value.pwd.as_os_str().is_empty() {
            inner.cwd.value.pwd = default_cwd.to_path_buf();
        }
        let mut notes = Vec::new();
        let mut cwd = inner.cwd.value.clone();
        if !cwd.pwd.is_dir() {
            notes.push(format!(
                "the directory {} no longer exists: this command runs in {} instead",
                cwd.pwd.display(),
                default_cwd.display()
            ));
            cwd.pwd = default_cwd.to_path_buf();
        }
        let exports: BTreeMap<String, OsString> = live(&inner.exports);
        let functions = live(&inner.functions);
        let aliases = live(&inner.aliases);
        let base = Snapshot {
            cwd,
            exports,
            functions,
            aliases,
        };
        let env = base
            .exports
            .iter()
            .map(|(k, v)| (OsString::from(k), v.clone()))
            .collect();
        let prelude = prelude_script(&base);
        Launch {
            seq,
            base,
            env,
            prelude,
            notes,
        }
    }

    /// Publishes the changes of a command launched with sequence `seq`, which
    /// started from `base` and ended with `end`. Returns whether anything
    /// changed.
    pub fn publish(&self, seq: u64, base: &Snapshot, end: &Snapshot) -> bool {
        let mut inner = self.inner.lock();
        let mut changed = false;
        if end.cwd != base.cwd && seq > inner.cwd.seq {
            inner.cwd = Field {
                value: end.cwd.clone(),
                seq,
            };
            changed = true;
        }
        changed |= apply(&mut inner.exports, seq, &base.exports, &end.exports);
        changed |= apply(&mut inner.functions, seq, &base.functions, &end.functions);
        changed |= apply(&mut inner.aliases, seq, &base.aliases, &end.aliases);
        if changed {
            self.generation.fetch_add(1, Ordering::SeqCst);
        }
        changed
    }

    /// Definitions of the aliases and functions among `names`, as bash
    /// prints them (empty when none is defined).
    pub fn describe(&self, names: &[String]) -> String {
        let inner = self.inner.lock();
        let mut out = String::new();
        for name in names {
            if let Some(Field {
                value: Some(text), ..
            }) = inner.aliases.get(name)
            {
                out.push_str(&String::from_utf8_lossy(text));
            }
            if let Some(Field {
                value: Some(text), ..
            }) = inner.functions.get(name)
            {
                out.push_str(&String::from_utf8_lossy(text));
            }
        }
        out
    }

    /// What an approval of a command depends on: its definitions, the
    /// directory and `PATH`. Equal fingerprints mean the command runs as
    /// described.
    pub fn fingerprint(&self, names: &[String]) -> String {
        let defs = self.describe(names);
        let inner = self.inner.lock();
        let path = inner
            .exports
            .get("PATH")
            .and_then(|f| f.value.clone())
            .unwrap_or_default();
        format!(
            "{defs}\u{0}{}\u{0}{}",
            inner.cwd.value.pwd.display(),
            path.to_string_lossy()
        )
    }
}

fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// The live (set) entries of a field map.
fn live<V: Clone>(fields: &BTreeMap<String, Field<Option<V>>>) -> BTreeMap<String, V> {
    fields
        .iter()
        .filter_map(|(k, f)| f.value.clone().map(|v| (k.clone(), v)))
        .collect()
}

/// Applies the differences `base` → `end` to `fields`, each name written
/// only when `seq` is newer than the command that last wrote it.
fn apply<V: Clone + PartialEq>(
    fields: &mut BTreeMap<String, Field<Option<V>>>,
    seq: u64,
    base: &BTreeMap<String, V>,
    end: &BTreeMap<String, V>,
) -> bool {
    let mut changed = false;
    for (name, value) in end {
        if base.get(name) != Some(value) {
            changed |= set_field(fields, name, seq, Some(value.clone()));
        }
    }
    for name in base.keys() {
        if !end.contains_key(name) {
            changed |= set_field(fields, name, seq, None);
        }
    }
    changed
}

fn set_field<V>(
    fields: &mut BTreeMap<String, Field<Option<V>>>,
    name: &str,
    seq: u64,
    value: Option<V>,
) -> bool {
    let field = fields.entry(name.to_string()).or_insert(Field {
        value: None,
        seq: 0,
    });
    if seq > field.seq {
        field.value = value;
        field.seq = seq;
        return true;
    }
    false
}

/// Quoted for bash: single quotes, with `'` written `'\''`.
fn quote(bytes: &[u8]) -> Vec<u8> {
    let mut out = vec![b'\''];
    for &b in bytes {
        if b == b'\'' {
            out.extend_from_slice(b"'\\''");
        } else {
            out.push(b);
        }
    }
    out.push(b'\'');
    out
}

fn prelude_script(base: &Snapshot) -> Vec<u8> {
    let mut s = b"shopt -s expand_aliases 2>/dev/null\n".to_vec();
    if let Some(old) = &base.cwd.oldpwd {
        s.extend_from_slice(b"OLDPWD=");
        s.extend(quote(old.as_os_str().as_bytes()));
        s.push(b'\n');
    }
    for alias in base.aliases.values() {
        s.extend_from_slice(alias);
        if !alias.ends_with(b"\n") {
            s.push(b'\n');
        }
    }
    for function in base.functions.values() {
        s.extend_from_slice(function);
        if !function.ends_with(b"\n") {
            s.push(b'\n');
        }
    }
    s
}

/// Parses the state a command recorded at its end: NUL-separated records,
/// closed by `end`. `None` when the record is incomplete or unknown.
pub fn parse_dump(bytes: &[u8]) -> Option<Snapshot> {
    let mut parts = bytes.split(|b| *b == 0);
    let mut snap = Snapshot::default();
    let mut cwd_seen = false;
    let mut ended = false;
    while let Some(kind) = parts.next() {
        match kind {
            b"cwd" => {
                snap.cwd.pwd = PathBuf::from(OsStr::from_bytes(parts.next()?));
                cwd_seen = true;
            }
            b"oldpwd" => {
                snap.cwd.oldpwd = Some(PathBuf::from(OsStr::from_bytes(parts.next()?)));
            }
            b"oldpwd-unset" => snap.cwd.oldpwd = None,
            b"env" => {
                let name = String::from_utf8(parts.next()?.to_vec()).ok()?;
                let value = OsString::from_vec(parts.next()?.to_vec());
                if valid_name(&name) {
                    snap.exports.insert(name, value);
                }
            }
            b"fn" => {
                let name = String::from_utf8(parts.next()?.to_vec()).ok()?;
                snap.functions.insert(name, parts.next()?.to_vec());
            }
            b"alias" => {
                let name = String::from_utf8(parts.next()?.to_vec()).ok()?;
                snap.aliases.insert(name, parts.next()?.to_vec());
            }
            b"end" => {
                ended = true;
                break;
            }
            _ => return None,
        }
    }
    (ended && cwd_seen).then_some(snap)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(dir: &str) -> Snapshot {
        Snapshot {
            cwd: Cwd {
                pwd: PathBuf::from(dir),
                oldpwd: None,
            },
            ..Snapshot::default()
        }
    }

    fn with_cwd(dir: &str) -> Snapshot {
        base(dir)
    }

    #[test]
    fn the_latest_launch_wins_whatever_the_order_of_ends() {
        // Older (1) and newer (2) both change the directory.
        let st = ShellState::from_env();
        let l1 = st.launch(Path::new("/"));
        let l2 = st.launch(Path::new("/"));
        assert!(st.publish(l2.seq, &l2.base, &with_cwd("/etc")));
        assert!(
            !st.publish(l1.seq, &l1.base, &with_cwd("/usr")),
            "older ends late"
        );
        assert_eq!(
            st.launch(Path::new("/")).base.cwd.pwd,
            PathBuf::from("/etc")
        );

        // Same, the older ending first: the newer one still wins once it ends.
        let st = ShellState::from_env();
        let l1 = st.launch(Path::new("/"));
        let l2 = st.launch(Path::new("/"));
        assert!(st.publish(l1.seq, &l1.base, &with_cwd("/usr")));
        assert!(st.publish(l2.seq, &l2.base, &with_cwd("/etc")));
        assert_eq!(
            st.launch(Path::new("/")).base.cwd.pwd,
            PathBuf::from("/etc")
        );
    }

    #[test]
    fn an_unchanged_field_is_not_written() {
        let st = ShellState::from_env();
        let old = st.launch(Path::new("/"));
        let newer = st.launch(Path::new("/"));
        // The newer command changes the directory...
        assert!(st.publish(newer.seq, &newer.base, &with_cwd("/usr/bin")));
        // ...and the older one, which did not touch it, changes nothing.
        assert!(!st.publish(old.seq, &old.base, &old.base));
        assert_eq!(
            st.launch(Path::new("/")).base.cwd.pwd,
            PathBuf::from("/usr/bin")
        );
    }

    #[test]
    fn removed_and_added_exports_are_published_and_managed_names_are_not_taken() {
        let st = ShellState::from_env();
        let l = st.launch(Path::new("/"));
        assert!(!l.base.exports.contains_key("PWD"));
        assert!(!l.base.exports.contains_key("SHLVL"));
        let mut end = l.base.clone();
        end.exports.insert("NEW".into(), "1".into());
        end.exports.remove("HOME");
        assert!(st.publish(l.seq, &l.base, &end));
        let next = st.launch(Path::new("/"));
        assert_eq!(next.base.exports.get("NEW"), Some(&OsString::from("1")));
        assert!(!next.base.exports.contains_key("HOME"));
    }

    #[test]
    fn the_dump_is_parsed_and_an_incomplete_one_is_refused() {
        let text = b"cwd\0/x\0oldpwd-unset\0env\0A\0a b\nc\0fn\0f\0f () \n{ \n    :\n}\n\0alias\0ll\0alias ll='ls'\n\0end\0";
        let snap = parse_dump(text).expect("complete");
        assert_eq!(snap.cwd.pwd, PathBuf::from("/x"));
        assert_eq!(snap.exports.get("A"), Some(&OsString::from("a b\nc")));
        assert!(snap.functions.contains_key("f"));
        assert!(snap.aliases.contains_key("ll"));
        assert!(parse_dump(&text[..text.len() - 4]).is_none(), "no end");
        assert!(parse_dump(b"").is_none());
    }

    #[test]
    fn quoting_survives_quotes_and_newlines() {
        assert_eq!(quote(b"it's"), b"'it'\\''s'".to_vec());
    }
}
