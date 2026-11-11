//! MCP servers, as every frontend reads them: the person's own, in
//! `<home>/mcp.toml`, and the project's, declared under `[[mcp.servers]]` in
//! its `bricks.toml`. A server a repository declares is connected only once
//! the person approved it, as declared: a changed declaration (command,
//! arguments, URL, headers…) is asked again. Approvals are kept in
//! `<home>/mcp-approvals.json`, never in the repository.
//!
//! ```toml
//! # ~/.bricks/mcp.toml
//! [[servers]]
//! name = "files"
//! command = "npx"
//! args = ["-y", "@modelcontextprotocol/server-filesystem", "."]
//!
//! [[servers]]
//! name = "docs"
//! type = "http"
//! url = "https://mcp.example.com/mcp"
//! headers = { Authorization = "Bearer ${DOCS_TOKEN}" }
//! enabled = false
//! ```

use bricks_mcp::McpServerConfig;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One server and whether it is used.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerEntry {
    #[serde(flatten)]
    pub config: McpServerConfig,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

impl ServerEntry {
    pub fn new(config: McpServerConfig) -> Self {
        Self {
            config,
            enabled: true,
        }
    }

    /// What the approval of a project server covers: everything that says
    /// what runs or where it connects.
    pub fn fingerprint(&self) -> String {
        // Keys sorted at every level: maps (env, headers) have no order.
        let text = serde_json::to_value(&self.config)
            .map(|v| canonical(&v))
            .unwrap_or_default();
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in text.bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        format!("{hash:016x}")
    }
}

/// JSON text with object keys sorted, at every level.
fn canonical(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let fields: Vec<String> = keys
                .into_iter()
                .map(|k| format!("{}:{}", serde_json::Value::String(k.clone()), canonical(&map[k])))
                .collect();
            format!("{{{}}}", fields.join(","))
        }
        serde_json::Value::Array(items) => {
            format!("[{}]", items.iter().map(canonical).collect::<Vec<_>>().join(","))
        }
        other => other.to_string(),
    }
}

/// Where a server is declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// `<home>/mcp.toml`: the person's own.
    User,
    /// The project's `bricks.toml`.
    Project,
}

/// The person's decision on a server a project declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    /// Not decided yet, or declared differently since: not connected.
    Pending,
    Approved,
    Denied,
}

/// A server as listed: where it comes from and, for a project's, the decision.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerView {
    pub entry: ServerEntry,
    pub source: Source,
    /// `None` for the person's own servers.
    pub decision: Option<Decision>,
    /// Hidden by a server of the person's with the same name.
    pub shadowed: bool,
}

impl ServerView {
    /// Connected at the next session: enabled and, for a project's, approved.
    pub fn active(&self) -> bool {
        self.entry.enabled
            && !self.shadowed
            && match self.source {
                Source::User => true,
                Source::Project => self.decision == Some(Decision::Approved),
            }
    }
}

/// The servers of a home and a project.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Selection {
    pub servers: Vec<ServerView>,
    /// Files that could not be read, servers that are not valid.
    pub errors: Vec<String>,
}

impl Selection {
    /// The servers to connect, valid ones only.
    pub fn active(&self) -> Vec<McpServerConfig> {
        self.servers
            .iter()
            .filter(|s| s.active() && s.entry.config.validate().is_ok())
            .map(|s| s.entry.config.clone())
            .collect()
    }

    /// The project's servers that wait for the person's decision.
    pub fn pending(&self) -> Vec<&ServerView> {
        self.servers
            .iter()
            .filter(|s| s.source == Source::Project && s.decision == Some(Decision::Pending) && !s.shadowed)
            .collect()
    }
}

// ─── The person's file ───────────────────────────────────────────────────────

#[derive(Debug, Default, Serialize, Deserialize)]
struct UserFile {
    #[serde(default)]
    servers: Vec<ServerEntry>,
}

pub fn user_file(home: &Path) -> PathBuf {
    home.join("mcp.toml")
}

/// The person's servers; an absent file has none, an invalid one is an error.
pub fn read_user(home: &Path) -> Result<Vec<ServerEntry>, String> {
    let path = user_file(home);
    match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str::<UserFile>(&text)
            .map(|f| f.servers)
            .map_err(|e| format!("{}: {}", path.display(), e.message())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// A server as written in the file: the values left at their default go.
fn compact(entry: &ServerEntry) -> Result<toml::Value, String> {
    let mut value = toml::Value::try_from(entry).map_err(|e| e.to_string())?;
    let defaults = toml::Value::try_from(ServerEntry::new(McpServerConfig::stdio("", "", &[])))
        .map_err(|e| e.to_string())?;
    if let (Some(table), Some(defaults)) = (value.as_table_mut(), defaults.as_table()) {
        table.retain(|key, v| {
            key == "name"
                || match defaults.get(key) {
                    Some(d) => d != v,
                    None => !matches!(v, toml::Value::Array(a) if a.is_empty())
                        && !matches!(v, toml::Value::Table(t) if t.is_empty()),
                }
        });
        // An HTTP server needs its type; a stdio one is the default.
        if entry.config.server_type != "stdio" {
            table.insert("type".into(), toml::Value::String(entry.config.server_type.clone()));
        }
    }
    Ok(value)
}

fn write_user(home: &Path, servers: &[ServerEntry]) -> Result<(), String> {
    let path = user_file(home);
    let list = servers.iter().map(compact).collect::<Result<Vec<_>, _>>()?;
    let mut file = toml::map::Map::new();
    file.insert("servers".into(), toml::Value::Array(list));
    let body = toml::to_string_pretty(&toml::Value::Table(file)).map_err(|e| e.to_string())?;
    let text = format!("# MCP servers of Bricks (`bricks mcp`, or Settings › Extensions).\n\n{body}");
    std::fs::create_dir_all(home).map_err(|e| format!("{}: {e}", home.display()))?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Adds a server of the person's. Refused when it is not valid or its name
/// is taken.
pub fn add_user(home: &Path, entry: ServerEntry) -> Result<(), String> {
    entry.config.validate()?;
    let mut servers = read_user(home)?;
    if servers.iter().any(|s| s.config.name == entry.config.name) {
        return Err(format!("an MCP server named '{}' already exists", entry.config.name));
    }
    servers.push(entry);
    write_user(home, &servers)
}

/// Removes a server of the person's; `false` when there was none.
pub fn remove_user(home: &Path, name: &str) -> Result<bool, String> {
    let mut servers = read_user(home)?;
    let before = servers.len();
    servers.retain(|s| s.config.name != name);
    if servers.len() == before {
        return Ok(false);
    }
    write_user(home, &servers).map(|_| true)
}

/// Turns a server of the person's on or off.
pub fn set_enabled(home: &Path, name: &str, enabled: bool) -> Result<(), String> {
    let mut servers = read_user(home)?;
    let server = servers
        .iter_mut()
        .find(|s| s.config.name == name)
        .ok_or_else(|| format!("no MCP server named '{name}'"))?;
    server.enabled = enabled;
    write_user(home, &servers)
}

// ─── The project's servers and their approvals ───────────────────────────────

#[derive(Debug, Default, Deserialize)]
struct ProjectFile {
    #[serde(default)]
    mcp: Option<ProjectMcp>,
}

#[derive(Debug, Default, Deserialize)]
struct ProjectMcp {
    #[serde(default)]
    servers: Vec<ServerEntry>,
}

/// The servers `<project>/bricks.toml` declares.
pub fn read_project(project: &Path) -> Result<Vec<ServerEntry>, String> {
    let path = project.join("bricks.toml");
    match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str::<ProjectFile>(&text)
            .map(|f| f.mcp.map(|m| m.servers).unwrap_or_default())
            .map_err(|e| format!("{} [mcp]: {}", path.display(), e.message())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Approval {
    fingerprint: String,
    approved: bool,
}

pub fn approvals_file(home: &Path) -> PathBuf {
    home.join("mcp-approvals.json")
}

fn approval_key(project: &Path, name: &str) -> String {
    let project = std::fs::canonicalize(project).unwrap_or_else(|_| project.to_path_buf());
    format!("{}#{name}", project.display())
}

/// The decisions kept; an unreadable file is an error (never taken for none,
/// and never overwritten).
fn load_approvals(home: &Path) -> Result<BTreeMap<String, Approval>, String> {
    let path = approvals_file(home);
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// The decisions, for reading: an unreadable file decides nothing (every
/// project server waits).
fn read_approvals(home: &Path) -> BTreeMap<String, Approval> {
    load_approvals(home).unwrap_or_default()
}

/// The person's decision on the project's server `entry`, as declared now.
pub fn decision(home: &Path, project: &Path, entry: &ServerEntry) -> Decision {
    match read_approvals(home).get(&approval_key(project, &entry.config.name)) {
        Some(a) if a.fingerprint == entry.fingerprint() => {
            if a.approved {
                Decision::Approved
            } else {
                Decision::Denied
            }
        }
        _ => Decision::Pending,
    }
}

/// Records the person's decision on the project's server `name`, as it is
/// declared now.
pub fn decide(home: &Path, project: &Path, name: &str, approved: bool) -> Result<(), String> {
    let entry = read_project(project)?
        .into_iter()
        .find(|s| s.config.name == name)
        .ok_or_else(|| format!("the project declares no MCP server named '{name}'"))?;
    let mut approvals = load_approvals(home)?;
    approvals.insert(
        approval_key(project, name),
        Approval {
            fingerprint: entry.fingerprint(),
            approved,
        },
    );
    let text = serde_json::to_string_pretty(&approvals).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(home).map_err(|e| e.to_string())?;
    let path = approvals_file(home);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

// ─── Both, as one selection ──────────────────────────────────────────────────

/// The servers of `home`, then those of `project` (when given). A project
/// server named like one of the person's is hidden by it.
pub fn select(home: &Path, project: Option<&Path>) -> Selection {
    let mut out = Selection::default();
    match read_user(home) {
        Ok(servers) => out.servers.extend(servers.into_iter().map(|entry| ServerView {
            entry,
            source: Source::User,
            decision: None,
            shadowed: false,
        })),
        Err(e) => out.errors.push(e),
    }
    if let Some(project) = project {
        match read_project(project) {
            Ok(servers) => {
                for entry in servers {
                    let shadowed = out
                        .servers
                        .iter()
                        .any(|s| s.source == Source::User && s.entry.config.name == entry.config.name);
                    let decision = Some(decision(home, project, &entry));
                    out.servers.push(ServerView {
                        entry,
                        source: Source::Project,
                        decision,
                        shadowed,
                    });
                }
            }
            Err(e) => out.errors.push(e),
        }
    }
    for s in &out.servers {
        if s.entry.enabled
            && let Err(e) = s.entry.config.validate()
        {
            out.errors.push(e);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stdio(name: &str) -> ServerEntry {
        ServerEntry::new(McpServerConfig::stdio(name, "echo", &["hi"]))
    }

    #[test]
    fn the_persons_servers_are_added_listed_turned_off_and_removed() {
        let home = tempfile::tempdir().unwrap();
        add_user(home.path(), stdio("a")).unwrap();
        add_user(home.path(), ServerEntry::new(McpServerConfig::http("b", "https://x.test/mcp"))).unwrap();
        assert!(add_user(home.path(), stdio("a")).is_err(), "names are unique");
        assert!(
            add_user(home.path(), ServerEntry::new(McpServerConfig::stdio("c", "", &[]))).is_err(),
            "invalid servers are refused"
        );
        set_enabled(home.path(), "b", false).unwrap();
        let sel = select(home.path(), None);
        assert_eq!(sel.servers.len(), 2);
        assert_eq!(sel.active().len(), 1, "a turned-off server is not connected");
        assert!(remove_user(home.path(), "a").unwrap());
        assert!(!remove_user(home.path(), "a").unwrap());
        let text = std::fs::read_to_string(user_file(home.path())).unwrap();
        assert!(text.contains("name = \"b\"") && text.contains("enabled = false"), "{text}");
    }

    #[test]
    fn a_project_server_waits_for_approval_and_a_change_asks_again() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let toml = "[context]\nx = 1\n\n[[mcp.servers]]\nname = \"repo\"\ncommand = \"node\"\nargs = [\"server.js\"]\n";
        std::fs::write(project.path().join("bricks.toml"), toml).unwrap();
        let sel = select(home.path(), Some(project.path()));
        assert_eq!(sel.pending().len(), 1);
        assert!(sel.active().is_empty(), "not connected before approval");

        decide(home.path(), project.path(), "repo", true).unwrap();
        assert_eq!(select(home.path(), Some(project.path())).active().len(), 1);

        // The declaration changes: asked again.
        std::fs::write(project.path().join("bricks.toml"), toml.replace("server.js", "other.js")).unwrap();
        let sel = select(home.path(), Some(project.path()));
        assert_eq!(sel.pending().len(), 1);
        assert!(sel.active().is_empty());

        decide(home.path(), project.path(), "repo", false).unwrap();
        let sel = select(home.path(), Some(project.path()));
        assert_eq!(sel.servers[0].decision, Some(Decision::Denied));
        assert!(sel.active().is_empty() && sel.pending().is_empty());
    }

    #[test]
    fn the_persons_server_hides_a_project_server_of_the_same_name() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        add_user(home.path(), stdio("same")).unwrap();
        std::fs::write(
            project.path().join("bricks.toml"),
            "[[mcp.servers]]\nname = \"same\"\ncommand = \"evil\"\n",
        )
        .unwrap();
        let sel = select(home.path(), Some(project.path()));
        let active = sel.active();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].command.as_deref(), Some("echo"));
        assert!(sel.pending().is_empty());
    }

    #[test]
    fn unreadable_files_are_reported_not_taken_for_empty() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(user_file(home.path()), "[[servers]\nname=").unwrap();
        let sel = select(home.path(), None);
        assert_eq!(sel.errors.len(), 1, "{:?}", sel.errors);
        assert!(add_user(home.path(), stdio("x")).is_err(), "an invalid file is never overwritten");
    }
}
