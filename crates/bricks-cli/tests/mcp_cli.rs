//! `bricks mcp` through the real binary, with a temporary Bricks home: the
//! person's servers (add, list, test, turn off and on, remove), and a
//! project's server, which waits for approval and does not run before it.
//! The server under test is a small MCP stdio server written next to the
//! project; it needs `python3` on the PATH.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// Answers `initialize` and `tools/list` over stdio, like a legacy server.
const STDIO_SERVER: &str = r#"
import json, sys
tools = [
    {"name": "add", "description": "Add two numbers",
     "inputSchema": {"type": "object", "properties": {"a": {"type": "number"}, "b": {"type": "number"}}}},
    {"name": "hello", "description": "Say hello", "inputSchema": {"type": "object"}},
]
def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()
for line in sys.stdin:
    try:
        msg = json.loads(line)
    except Exception:
        continue
    mid, method = msg.get("id"), msg.get("method")
    if mid is None:
        continue
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": mid, "result": {"protocolVersion": "2025-11-25",
              "capabilities": {"tools": {}}, "serverInfo": {"name": "test-stdio", "version": "1"}}})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": mid, "result": {"tools": tools}})
    else:
        send({"jsonrpc": "2.0", "id": mid, "error": {"code": -32601, "message": "Method not found"}})
"#;

struct Fixture {
    _root: tempfile::TempDir,
    home: PathBuf,
    project: PathBuf,
    server: PathBuf,
}

fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().canonicalize().unwrap();
    let home = base.join("bricks-home");
    let project = base.join("project");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    let server = base.join("stdio_server.py");
    std::fs::write(&server, STDIO_SERVER).unwrap();
    Fixture {
        _root: root,
        home,
        project,
        server,
    }
}

/// `bricks <args>` started from `dir`, with `home` as its Bricks home.
fn bricks(dir: &Path, home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bricks"))
        .args(args)
        .current_dir(dir)
        .env("BRICKS_HOME", home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap()
}

fn code(out: &Output) -> i32 {
    out.status.code().unwrap_or(-1)
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// `bricks mcp list --json` as objects, one per server.
fn listed(f: &Fixture) -> Vec<Value> {
    let out = bricks(&f.project, &f.home, &["mcp", "list", "--json"]);
    assert_eq!(code(&out), 0, "list failed: {}", stderr(&out));
    stdout(&out)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON `{l}`: {e}")))
        .collect()
}

fn find<'a>(servers: &'a [Value], name: &str) -> Option<&'a Value> {
    servers.iter().find(|s| s["name"] == name)
}

fn add_stdio(f: &Fixture, name: &str) -> Output {
    let server = f.server.display().to_string();
    bricks(
        &f.project,
        &f.home,
        &["mcp", "add", name, "--", "python3", &server],
    )
}

#[test]
fn a_person_server_is_added_listed_tested_switched_and_removed() {
    let f = fixture();

    let added = add_stdio(&f, "files");
    assert_eq!(code(&added), 0, "add failed: {}", stderr(&added));
    assert!(
        stdout(&added).contains("Added 'files'"),
        "{}",
        stdout(&added)
    );
    assert!(
        f.home.join("mcp.toml").is_file(),
        "the server is not in mcp.toml"
    );

    let servers = listed(&f);
    let files = find(&servers, "files").expect("files is listed");
    assert_eq!(files["source"], "user");
    assert_eq!(files["type"], "stdio");
    assert_eq!(files["enabled"], true);
    assert_eq!(files["active"], true);
    assert!(
        files["target"].as_str().unwrap().starts_with("python3 "),
        "{files}"
    );

    let tested = bricks(&f.project, &f.home, &["mcp", "test", "files"]);
    assert_eq!(code(&tested), 0, "test failed: {}", stderr(&tested));
    assert!(
        stdout(&tested).contains("answers: 2 tool(s)"),
        "{}",
        stdout(&tested)
    );
    assert!(
        stdout(&tested).contains("add"),
        "the tool `add` is not listed: {}",
        stdout(&tested)
    );

    let off = bricks(&f.project, &f.home, &["mcp", "disable", "files"]);
    assert_eq!(code(&off), 0, "disable failed: {}", stderr(&off));
    let files = listed(&f)
        .into_iter()
        .find(|s| s["name"] == "files")
        .unwrap();
    assert_eq!(
        (files["enabled"].clone(), files["active"].clone()),
        (Value::Bool(false), Value::Bool(false))
    );

    let on = bricks(&f.project, &f.home, &["mcp", "enable", "files"]);
    assert_eq!(code(&on), 0, "enable failed: {}", stderr(&on));
    assert_eq!(find(&listed(&f), "files").unwrap()["active"], true);

    let removed = bricks(&f.project, &f.home, &["mcp", "remove", "files"]);
    assert_eq!(code(&removed), 0, "remove failed: {}", stderr(&removed));
    assert!(
        find(&listed(&f), "files").is_none(),
        "files is still listed after remove"
    );
}

#[test]
fn an_unknown_name_fails_with_exit_1() {
    let f = fixture();
    let tested = bricks(&f.project, &f.home, &["mcp", "test", "nobody"]);
    assert_eq!(
        code(&tested),
        1,
        "stdout: {} stderr: {}",
        stdout(&tested),
        stderr(&tested)
    );
    assert!(
        stderr(&tested).contains("no MCP server named 'nobody'"),
        "{}",
        stderr(&tested)
    );

    let removed = bricks(&f.project, &f.home, &["mcp", "remove", "nobody"]);
    assert_eq!(
        code(&removed),
        1,
        "remove of an unknown name: {}",
        stderr(&removed)
    );
    let disabled = bricks(&f.project, &f.home, &["mcp", "disable", "nobody"]);
    assert_eq!(
        code(&disabled),
        1,
        "disable of an unknown name: {}",
        stderr(&disabled)
    );
}

#[test]
fn a_project_server_waits_for_approval_and_does_not_run_before_it() {
    let f = fixture();
    let server = f.server.display().to_string();
    std::fs::write(
        f.project.join("bricks.toml"),
        format!(
            "[agent]\nmodel = \"local/m\"\n\n[[mcp.servers]]\nname = \"repo\"\ncommand = \"python3\"\nargs = [\"{server}\"]\n"
        ),
    )
    .unwrap();

    let servers = listed(&f);
    let repo = find(&servers, "repo").expect("the project's server is listed");
    assert_eq!(repo["source"], "project");
    assert_eq!(repo["decision"], "pending");
    assert_eq!(repo["active"], false);

    let text = bricks(&f.project, &f.home, &["mcp", "list"]);
    assert!(
        stdout(&text).contains("awaiting approval"),
        "{}",
        stdout(&text)
    );

    let refused = bricks(&f.project, &f.home, &["mcp", "test", "repo"]);
    assert_eq!(
        code(&refused),
        1,
        "a project server ran before approval: {}",
        stdout(&refused)
    );
    assert!(
        stderr(&refused).contains("not approved"),
        "{}",
        stderr(&refused)
    );
    assert!(
        stdout(&refused).is_empty(),
        "the server answered before approval: {}",
        stdout(&refused)
    );

    let approved = bricks(&f.project, &f.home, &["mcp", "approve", "repo"]);
    assert_eq!(code(&approved), 0, "approve failed: {}", stderr(&approved));
    assert_eq!(find(&listed(&f), "repo").unwrap()["active"], true);
    let tested = bricks(&f.project, &f.home, &["mcp", "test", "repo"]);
    assert_eq!(
        code(&tested),
        0,
        "test after approval failed: {}",
        stderr(&tested)
    );

    let denied = bricks(&f.project, &f.home, &["mcp", "deny", "repo"]);
    assert_eq!(code(&denied), 0, "deny failed: {}", stderr(&denied));
    let repo = find(&listed(&f), "repo").cloned().unwrap();
    assert_eq!(
        (repo["decision"].clone(), repo["active"].clone()),
        (Value::String("denied".into()), Value::Bool(false))
    );
    assert_eq!(
        code(&bricks(&f.project, &f.home, &["mcp", "test", "repo"])),
        1
    );
}
