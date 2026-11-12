//! A server that announces `notifications/tools/list_changed` is re-read once,
//! and every consumer of the manager learns of the change: a tool call made
//! in between must not swallow it. A stdio server written for the test
//! offers `add`, and `grow`, which adds `bonus` and announces the change.

use bricks_mcp::{McpManager, McpServerConfig};
use serde_json::json;
use std::time::Duration;

const SERVER: &str = r#"
import json, sys
tools = [
    {"name": "add", "description": "Add", "inputSchema": {"type": "object"}},
    {"name": "grow", "description": "Add a tool named bonus", "inputSchema": {"type": "object"}},
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
              "capabilities": {"tools": {"listChanged": True}}, "serverInfo": {"name": "grow", "version": "1"}}})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": mid, "result": {"tools": tools}})
    elif method == "tools/call":
        name = msg.get("params", {}).get("name")
        if name == "grow":
            if not any(t["name"] == "bonus" for t in tools):
                tools.append({"name": "bonus", "description": "Added at run time", "inputSchema": {"type": "object"}})
            send({"jsonrpc": "2.0", "id": mid, "result": {"content": [{"type": "text", "text": "grown"}]}})
            send({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"})
        else:
            send({"jsonrpc": "2.0", "id": mid, "result": {"content": [{"type": "text", "text": "ok"}]}})
    else:
        send({"jsonrpc": "2.0", "id": mid, "error": {"code": -32601, "message": "Method not found"}})
"#;

fn names(tools: &[(String, bricks_mcp::McpToolDef)]) -> Vec<String> {
    let mut n: Vec<String> = tools.iter().map(|(_, t)| t.name.clone()).collect();
    n.sort();
    n
}

#[tokio::test]
async fn a_tool_list_change_is_still_reported_after_a_tool_call_in_between() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("grow_server.py");
    std::fs::write(&script, SERVER).unwrap();
    let config = McpServerConfig::stdio("grow", "python3", &[script.to_str().unwrap()]);
    let manager = McpManager::connect(&[config]).await;

    assert_eq!(names(&manager.tools().await), vec!["add", "grow"]);
    let before = manager.tools_generation().await;
    manager
        .call("grow", "grow", Some(json!({})), None)
        .await
        .unwrap();

    // The notification follows the answer; wait for the connection to record it.
    let client = manager
        .client("grow")
        .await
        .expect("the server is connected");
    let mut announced = false;
    for _ in 0..200 {
        if client.tools_changed() {
            announced = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        announced,
        "the server's list_changed notification never arrived"
    );

    // A call to another tool re-reads the list on its way (McpManager::call), which clears the flag.
    manager
        .call("grow", "add", Some(json!({"a": 1, "b": 2})), None)
        .await
        .unwrap();
    assert_eq!(names(&manager.tools().await), vec!["add", "bonus", "grow"]);

    // Whoever re-read the list, the generation moved: every agent sharing this manager
    // compares it with the one it built its tools from, and rebuilds them.
    manager.refresh_changed().await;
    assert_ne!(
        manager.tools_generation().await,
        before,
        "the change was consumed by a tool call: the agents sharing this manager keep the old list"
    );
    manager.close().await;
}
