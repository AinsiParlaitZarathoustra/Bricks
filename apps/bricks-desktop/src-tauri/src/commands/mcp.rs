//! The MCP servers of Bricks, as the settings show them: the person's
//! (`<home>/mcp.toml`, shared with `bricks mcp`) and those of the project in
//! view, which connect only once approved. Changes reach the conversations
//! opened afterwards.

use crate::contract::{
    IpcResult, McpAppCallResult, McpAppView, McpServerInput, McpServerItem, McpServersView, McpTestResult,
};
use crate::engine::AppState;
use crate::services::{invalid, not_found};
use bricks_config::mcp::{self, Decision, ServerEntry, Selection, Source};
use bricks_mcp::{McpManager, McpServerConfig, McpServerStatus};
use std::path::PathBuf;
use tauri::State;

fn home() -> PathBuf {
    bricks_config::bricks_home()
}

/// The folder of a project of Bricks (none for the conversations without one).
fn project_dir(state: &AppState, project_id: Option<&str>) -> IpcResult<Option<PathBuf>> {
    let Some(id) = project_id else {
        return Ok(None);
    };
    let project = state.projects.get(id)?;
    Ok((!project.chats).then(|| PathBuf::from(project.path)))
}

fn target(config: &McpServerConfig) -> String {
    match &config.url {
        Some(url) => url.clone(),
        None => std::iter::once(config.command.clone().unwrap_or_default())
            .chain(config.args.iter().cloned())
            .collect::<Vec<_>>()
            .join(" "),
    }
}

fn view(selection: Selection, project: Option<PathBuf>) -> McpServersView {
    McpServersView {
        project: project.map(|p| p.display().to_string()),
        servers: selection
            .servers
            .iter()
            .map(|s| McpServerItem {
                name: s.entry.config.name.clone(),
                source: match s.source {
                    Source::User => "user".into(),
                    Source::Project => "project".into(),
                },
                transport: if s.entry.config.url.is_some() { "http" } else { "stdio" }.into(),
                target: target(&s.entry.config),
                enabled: s.entry.enabled,
                decision: s.decision.map(|d| {
                    match d {
                        Decision::Pending => "pending",
                        Decision::Approved => "approved",
                        Decision::Denied => "denied",
                    }
                    .into()
                }),
                active: s.active(),
                shadowed: s.shadowed,
            })
            .collect(),
        errors: selection.errors,
    }
}

fn current(state: &AppState, project_id: Option<&str>) -> IpcResult<McpServersView> {
    let project = project_dir(state, project_id)?;
    Ok(view(mcp::select(&home(), project.as_deref()), project))
}

fn rejected(message: String) -> crate::contract::IpcError {
    crate::services::error(crate::contract::IpcErrorCode::Rejected, message)
}

#[tauri::command]
pub fn mcp_servers(state: State<'_, AppState>, project_id: Option<String>) -> IpcResult<McpServersView> {
    current(&state, project_id.as_deref())
}

/// Splits a command line as a shell would for plain words and quotes
/// ("…" and '…'); nothing is expanded.
pub(crate) fn split_command(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    for c in line.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => word.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                in_word = true;
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            (None, c) => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if quote.is_some() {
        return Err("a quote is not closed".into());
    }
    if in_word {
        words.push(word);
    }
    Ok(words)
}

fn pairs(lines: &[String], what: &str) -> IpcResult<std::collections::HashMap<String, String>> {
    lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            l.split_once('=')
                .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
                .filter(|(k, _)| !k.is_empty())
                .ok_or_else(|| invalid(format!("{what} « {l} »: NAME=VALUE")))
        })
        .collect()
}

#[tauri::command]
pub fn mcp_add(
    state: State<'_, AppState>,
    input: McpServerInput,
    project_id: Option<String>,
) -> IpcResult<McpServersView> {
    let name = input.name.trim().to_string();
    let mut config = if input.transport == "http" {
        McpServerConfig::http(&name, input.url.trim())
    } else {
        let words = split_command(&input.command).map_err(invalid)?;
        let Some((program, args)) = words.split_first() else {
            return Err(invalid("the command is empty"));
        };
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        McpServerConfig::stdio(&name, program, &args)
    };
    config.headers = pairs(&input.headers, "header")?;
    config.env = pairs(&input.env, "env")?;
    mcp::add_user(&home(), ServerEntry::new(config)).map_err(invalid)?;
    current(&state, project_id.as_deref())
}

#[tauri::command]
pub fn mcp_remove(state: State<'_, AppState>, name: String, project_id: Option<String>) -> IpcResult<McpServersView> {
    if !mcp::remove_user(&home(), &name).map_err(rejected)? {
        return Err(not_found(format!("no MCP server of yours named '{name}'")));
    }
    current(&state, project_id.as_deref())
}

#[tauri::command]
pub fn mcp_set_enabled(
    state: State<'_, AppState>,
    name: String,
    enabled: bool,
    project_id: Option<String>,
) -> IpcResult<McpServersView> {
    mcp::set_enabled(&home(), &name, enabled).map_err(rejected)?;
    current(&state, project_id.as_deref())
}

/// Approves or refuses a server the project declares, as declared now.
#[tauri::command]
pub fn mcp_decide(
    state: State<'_, AppState>,
    project_id: String,
    name: String,
    approve: bool,
) -> IpcResult<McpServersView> {
    let project = project_dir(&state, Some(&project_id))?
        .ok_or_else(|| invalid("the conversations without a project declare no server"))?;
    mcp::decide(&home(), &project, &name, approve).map_err(rejected)?;
    current(&state, Some(&project_id))
}

/// The configuration of the server `name` that may run: one of the person's,
/// or a project's once approved.
fn runnable(state: &AppState, name: &str, project_id: Option<&str>) -> IpcResult<McpServerConfig> {
    let project = project_dir(state, project_id)?;
    let server = mcp::select(&home(), project.as_deref())
        .servers
        .into_iter()
        .find(|s| s.entry.config.name == name && !s.shadowed)
        .ok_or_else(|| not_found(format!("no MCP server named '{name}'")))?;
    if server.source == Source::Project && server.decision != Some(Decision::Approved) {
        return Err(rejected(format!("'{name}' is declared by the project: approve it first")));
    }
    Ok(server.entry.config)
}

/// One connection to `config`, or why it failed.
async fn connect(config: &McpServerConfig) -> Result<McpManager, String> {
    let manager = McpManager::connect(std::slice::from_ref(config)).await;
    match manager.statuses().await.into_iter().find(|(n, _)| *n == config.name) {
        Some((_, McpServerStatus::Connected)) => Ok(manager),
        Some((_, McpServerStatus::Error(e))) => Err(e),
        _ => Err(format!("'{}' is not connected", config.name)),
    }
}

/// Connects to a server and lists its tools (and those with an interface).
/// A project's server runs only once approved.
#[tauri::command]
pub async fn mcp_test(
    state: State<'_, AppState>,
    name: String,
    project_id: Option<String>,
) -> IpcResult<McpTestResult> {
    let config = runnable(&state, &name, project_id.as_deref())?;
    Ok(match connect(&config).await {
        Ok(manager) => {
            let tools = manager.tools().await;
            manager.close().await;
            McpTestResult {
                ok: true,
                apps: tools
                    .iter()
                    .filter(|(_, t)| t.ui_resource().is_some())
                    .map(|(_, t)| t.name.clone())
                    .collect(),
                tools: tools.into_iter().map(|(_, t)| t.name).collect(),
                error: None,
            }
        }
        Err(e) => McpTestResult {
            ok: false,
            tools: Vec::new(),
            apps: Vec::new(),
            error: Some(e),
        },
    })
}

/// The interface of `tool` (MCP Apps): the HTML of its `ui://` resource,
/// shown by the window in an isolated frame.
#[tauri::command]
pub async fn mcp_app_load(
    state: State<'_, AppState>,
    name: String,
    tool: String,
    project_id: Option<String>,
) -> IpcResult<McpAppView> {
    let config = runnable(&state, &name, project_id.as_deref())?;
    let manager = connect(&config).await.map_err(rejected)?;
    let result = async {
        let def = manager
            .tools()
            .await
            .into_iter()
            .map(|(_, t)| t)
            .find(|t| t.name == tool)
            .ok_or_else(|| not_found(format!("'{name}' has no tool '{tool}'")))?;
        let uri = def
            .ui_resource()
            .ok_or_else(|| invalid(format!("'{tool}' has no interface")))?
            .to_string();
        let client = manager
            .client(&name)
            .await
            .ok_or_else(|| rejected(format!("'{name}' is not connected")))?;
        let contents = client.read_resource(&uri).await.map_err(|e| rejected(e.to_string()))?;
        let html = contents
            .into_iter()
            .find_map(|c| match c {
                bricks_mcp::McpContent::Resource { text: Some(text), mime_type, .. }
                    if mime_type.as_deref().is_none_or(|m| m.starts_with("text/html")) =>
                {
                    Some(text)
                }
                _ => None,
            })
            .ok_or_else(|| rejected(format!("{uri} holds no HTML")))?;
        Ok(McpAppView { uri, html })
    }
    .await;
    manager.close().await;
    result
}

/// A call made by an interface: only to a tool of its own server.
#[tauri::command]
pub async fn mcp_app_call(
    state: State<'_, AppState>,
    name: String,
    tool: String,
    arguments: serde_json::Value,
    project_id: Option<String>,
) -> IpcResult<McpAppCallResult> {
    let config = runnable(&state, &name, project_id.as_deref())?;
    let manager = connect(&config).await.map_err(rejected)?;
    let known = manager.tools().await.iter().any(|(_, t)| t.name == tool);
    let result = if !known {
        Err(rejected(format!("'{tool}' is not a tool of '{name}'")))
    } else {
        manager
            .call(&name, &tool, Some(arguments), None)
            .await
            .map(|r| McpAppCallResult {
                is_error: r.is_error,
                text: r
                    .content
                    .iter()
                    .filter_map(|c| match c {
                        bricks_mcp::McpContent::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            })
            .map_err(|e| rejected(e.to_string()))
    };
    manager.close().await;
    result
}

#[cfg(test)]
mod tests {
    use super::split_command;

    #[test]
    fn a_command_line_splits_into_words_and_keeps_quoted_spaces() {
        assert_eq!(
            split_command(r#"npx -y "@scope/server one" '/a b/c' x"#).unwrap(),
            vec!["npx", "-y", "@scope/server one", "/a b/c", "x"]
        );
        assert_eq!(split_command(r#"run "" end"#).unwrap(), vec!["run", "", "end"]);
        assert!(split_command("a \"b").is_err());
        assert!(split_command("   ").unwrap().is_empty());
    }
}
