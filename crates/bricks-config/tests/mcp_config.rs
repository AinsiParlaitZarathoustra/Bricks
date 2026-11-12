//! The MCP configuration files: the person's `mcp.toml` round trip, the
//! fingerprint of a project's declaration, and the approvals keyed on the
//! project folder. Files only, under a temporary Bricks home.

use bricks_config::mcp::{
    self, Decision, ServerEntry, decide, decision, read_project, read_user, select,
};
use bricks_mcp::{McpLimits, McpServerConfig, ProtocolMode};
use std::collections::HashMap;
use std::path::Path;

fn env_pairs(n: usize, prefix: &str) -> HashMap<String, String> {
    (0..n)
        .map(|i| (format!("{prefix}_{i}"), format!("value-{i}")))
        .collect()
}

fn limits() -> McpLimits {
    McpLimits {
        connect_timeout_ms: 5_000,
        request_timeout_ms: 9_000,
        max_total_timeout_ms: 120_000,
        reset_on_progress: false,
        max_message_bytes: 4 * 1024 * 1024,
        max_in_flight: 3,
        mrtr_max_rounds: 2,
    }
}

fn stdio_with_everything() -> ServerEntry {
    let mut config = McpServerConfig::stdio("files", "node", &["server.js", "--root", "."])
        .with_protocol(ProtocolMode::Legacy)
        .with_limits(limits());
    config.env = env_pairs(5, "ENV");
    config.cwd = Some("/opt/servers/files".into());
    ServerEntry::new(config)
}

fn http_with_everything() -> ServerEntry {
    let mut config =
        McpServerConfig::http("docs", "https://mcp.example.com/mcp").with_limits(limits());
    config.headers = env_pairs(4, "X-Header");
    ServerEntry::new(config)
}

/// Writes a project's bricks.toml, with the given server tables.
fn write_project(dir: &Path, servers: &str) {
    std::fs::write(
        dir.join("bricks.toml"),
        format!("[agent]\nmodel = \"local/m\"\n\n{servers}"),
    )
    .unwrap();
}

const ENV_SERVER: &str = r#"[[mcp.servers]]
name = "repo"
command = "python3"
args = ["server.py"]
env = { A_KEY = "1", B_KEY = "2", C_KEY = "3", D_KEY = "4", E_KEY = "5", F_KEY = "6" }

[[mcp.servers]]
name = "remote"
type = "http"
url = "https://mcp.example.com/mcp"
headers = { Authorization = "Bearer ${TOKEN}", "X-One" = "1", "X-Two" = "2", "X-Three" = "3" }
"#;

#[test]
fn a_server_with_env_headers_cwd_and_limits_survives_the_round_trip() {
    let home = tempfile::tempdir().unwrap();
    let stdio = stdio_with_everything();
    let http = http_with_everything();
    mcp::add_user(home.path(), stdio.clone()).unwrap();
    mcp::add_user(home.path(), http.clone()).unwrap();
    mcp::set_enabled(home.path(), "docs", false).unwrap();

    let read = read_user(home.path()).unwrap();
    assert_eq!(read.len(), 2);
    let mut expected_http = http.clone();
    expected_http.enabled = false;
    assert_eq!(
        read[0], stdio,
        "stdio server changed on the way to the file and back"
    );
    assert_eq!(
        read[1], expected_http,
        "http server changed on the way to the file and back"
    );

    // Rewriting the file keeps everything: the values left at their default are not written.
    mcp::set_enabled(home.path(), "files", true).unwrap();
    assert_eq!(read_user(home.path()).unwrap()[0], stdio);
    let text = std::fs::read_to_string(mcp::user_file(home.path())).unwrap();
    assert!(text.contains("cwd = \"/opt/servers/files\""), "{text}");
    assert!(text.contains("protocol = \"legacy\""), "{text}");
}

#[test]
fn a_server_with_default_values_is_written_compactly() {
    let home = tempfile::tempdir().unwrap();
    mcp::add_user(
        home.path(),
        ServerEntry::new(McpServerConfig::stdio("plain", "srv", &[])),
    )
    .unwrap();
    let text = std::fs::read_to_string(mcp::user_file(home.path())).unwrap();
    assert!(
        !text.contains("limits"),
        "default limits are written: {text}"
    );
    assert!(
        !text.contains("env") && !text.contains("headers"),
        "empty tables are written: {text}"
    );
    assert_eq!(
        read_user(home.path()).unwrap()[0].config,
        McpServerConfig::stdio("plain", "srv", &[])
    );
}

#[test]
fn the_fingerprint_of_a_declaration_is_the_same_on_every_read() {
    // Several env and header keys: the HashMaps of each read iterate in their own order, so a
    // fingerprint built from the JSON text of the maps changes from one read to the next.
    let project = tempfile::tempdir().unwrap();
    write_project(project.path(), ENV_SERVER);
    let first: Vec<String> = read_project(project.path())
        .unwrap()
        .iter()
        .map(ServerEntry::fingerprint)
        .collect();
    for _ in 0..25 {
        let again: Vec<String> = read_project(project.path())
            .unwrap()
            .iter()
            .map(ServerEntry::fingerprint)
            .collect();
        assert_eq!(again, first, "the same declaration has another fingerprint");
    }
}

#[test]
fn an_approved_declaration_with_several_env_keys_stays_approved() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    write_project(project.path(), ENV_SERVER);
    decide(home.path(), project.path(), "repo", true).unwrap();
    for _ in 0..10 {
        let sel = select(home.path(), Some(project.path()));
        // Only `repo` was approved: `remote` rightly waits.
        assert!(
            sel.pending().iter().all(|s| s.entry.config.name != "repo"),
            "approved server asked again: {:?}",
            sel.errors
        );
        assert_eq!(
            sel.active().len(),
            1,
            "approved server not connected on a later read"
        );
    }
}

#[cfg(unix)]
#[test]
fn an_approval_given_through_a_symlink_or_a_trailing_dot_is_the_same_decision() {
    let root = tempfile::tempdir().unwrap();
    let real = root.path().join("repo");
    std::fs::create_dir_all(&real).unwrap();
    // No env or headers here: the fingerprint is not what this test looks at.
    write_project(
        &real,
        "[[mcp.servers]]\nname = \"repo\"\ncommand = \"python3\"\nargs = [\"server.py\"]\n",
    );
    let link = root.path().join("linked");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let home = tempfile::tempdir().unwrap();
    let entry = read_project(&real).unwrap().remove(0);

    decide(home.path(), &link, "repo", true).unwrap();
    assert_eq!(
        decision(home.path(), &real, &entry),
        Decision::Approved,
        "via the real folder"
    );
    assert_eq!(
        decision(home.path(), &link, &entry),
        Decision::Approved,
        "via the symlink"
    );
    assert_eq!(
        decision(home.path(), &real.join("."), &entry),
        Decision::Approved,
        "with a trailing `/.`"
    );
    assert_eq!(
        decision(home.path(), &link.join("."), &entry),
        Decision::Approved,
        "symlink and `/.`"
    );
    let slashed = format!("{}/", real.display());
    assert_eq!(
        decision(home.path(), Path::new(&slashed), &entry),
        Decision::Approved,
        "with a trailing `/`"
    );

    let sel = select(home.path(), Some(&link.join(".")));
    assert_eq!(
        sel.active().len(),
        1,
        "the server is not connected through another spelling of the folder"
    );
    assert_eq!(
        sel.servers
            .iter()
            .filter(|s| s.source == mcp::Source::Project)
            .count(),
        1
    );
}

#[test]
fn a_decision_does_not_erase_an_unreadable_approvals_file() {
    // The approvals of every project are in one file. A file that cannot be read must be an error,
    // not an empty list that the next decision overwrites.
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    write_project(project.path(), ENV_SERVER);
    let approvals = mcp::approvals_file(home.path());
    std::fs::write(&approvals, "{ this is not json").unwrap();

    let outcome = decide(home.path(), project.path(), "repo", true);
    let after = std::fs::read_to_string(&approvals).unwrap();
    assert!(
        outcome.is_err() || after == "{ this is not json",
        "an unreadable approvals file was overwritten by a decision: {after}"
    );
}
