//! `bricks mcp`: the MCP servers of Bricks. The person's are kept in
//! `~/.bricks/mcp.toml` (shared with the desktop application); a project's
//! are declared in its `bricks.toml` and connected only once approved here
//! (or in the desktop application).

use crate::args::{Global, McpCmd};
use crate::exit;
use bricks_config::mcp::{self, Decision, ServerEntry, ServerView, Source};
use bricks_mcp::{McpManager, McpServerConfig, McpServerStatus};
use std::path::Path;

pub async fn run(global: &Global, launch: &Path, command: McpCmd) -> i32 {
    let home = bricks_config::bricks_home();
    let project = crate::setup::workspace(global, launch).ok();
    let outcome = match command {
        McpCmd::List { json } => list(&home, project.as_deref(), json),
        McpCmd::Add {
            name,
            url,
            header,
            env,
            cwd,
            command,
        } => add(&home, launch, name, url, header, env, cwd, command),
        McpCmd::Remove { name } => match mcp::remove_user(&home, &name) {
            Ok(true) => {
                println!("Removed '{name}'.");
                Ok(())
            }
            Ok(false) => Err(format!("no MCP server of yours named '{name}'")),
            Err(e) => Err(e),
        },
        McpCmd::Enable { name } => mcp::set_enabled(&home, &name, true).map(|_| println!("'{name}' is on.")),
        McpCmd::Disable { name } => mcp::set_enabled(&home, &name, false).map(|_| println!("'{name}' is off.")),
        McpCmd::Approve { name } | McpCmd::Deny { name } if project.is_none() => {
            Err(format!("no project folder to decide on '{name}'"))
        }
        McpCmd::Approve { name } => mcp::decide(&home, project.as_deref().unwrap(), &name, true)
            .map(|_| println!("'{name}' is approved for this project, as declared now.")),
        McpCmd::Deny { name } => mcp::decide(&home, project.as_deref().unwrap(), &name, false)
            .map(|_| println!("'{name}' is refused for this project.")),
        McpCmd::Test { name } => return test(&home, project.as_deref(), &name).await,
    };
    match outcome {
        Ok(()) => exit::OK,
        Err(e) => {
            eprintln!("bricks: {e}");
            exit::FAILED
        }
    }
}

fn state(view: &ServerView) -> &'static str {
    if view.shadowed {
        return "hidden by yours";
    }
    match (view.source, view.decision, view.entry.enabled) {
        (_, _, false) => "off",
        (Source::Project, Some(Decision::Pending), _) => "awaiting approval",
        (Source::Project, Some(Decision::Denied), _) => "refused",
        _ => "on",
    }
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

fn list(home: &Path, project: Option<&Path>, json: bool) -> Result<(), String> {
    let sel = mcp::select(home, project);
    if json {
        for s in &sel.servers {
            let line = serde_json::json!({
                "name": s.entry.config.name,
                "source": s.source,
                "type": s.entry.config.server_type,
                "target": target(&s.entry.config),
                "enabled": s.entry.enabled,
                "decision": s.decision,
                "active": s.active(),
            });
            println!("{line}");
        }
    } else if sel.servers.is_empty() {
        println!(
            "No MCP server. Add one with `bricks mcp add <name> -- <command> [args…]` or \
             `bricks mcp add <name> --url <url>`."
        );
    } else {
        for s in &sel.servers {
            let from = match s.source {
                Source::User => "yours",
                Source::Project => "project",
            };
            println!(
                "{}  [{from}, {}]  {}",
                s.entry.config.name,
                state(s),
                target(&s.entry.config)
            );
        }
        if !sel.pending().is_empty() {
            println!("\nApprove a project's server with `bricks mcp approve <name>`.");
        }
    }
    for e in &sel.errors {
        eprintln!("bricks: {e}");
    }
    Ok(())
}

fn pairs(items: &[String], what: &str) -> Result<std::collections::HashMap<String, String>, String> {
    items
        .iter()
        .map(|item| {
            item.split_once('=')
                .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
                .filter(|(k, _)| !k.is_empty())
                .ok_or_else(|| format!("{what} `{item}`: expected NAME=VALUE"))
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn add(
    home: &Path,
    launch: &Path,
    name: String,
    url: Option<String>,
    header: Vec<String>,
    env: Vec<String>,
    cwd: Option<std::path::PathBuf>,
    command: Vec<String>,
) -> Result<(), String> {
    let mut config = match (url, command.split_first()) {
        (Some(url), None) => McpServerConfig::http(&name, url),
        (None, Some((program, args))) => {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            McpServerConfig::stdio(&name, program, &args)
        }
        (Some(_), Some(_)) => return Err("give either --url or a command, not both".into()),
        (None, None) => {
            return Err("give the server's command after `--`, or --url for an HTTP server".into());
        }
    };
    config.headers = pairs(&header, "header")?;
    config.env = pairs(&env, "env")?;
    config.cwd = cwd.map(|c| if c.is_absolute() { c } else { launch.join(c) });
    mcp::add_user(home, ServerEntry::new(config))?;
    println!("Added '{name}'. Check it with `bricks mcp test {name}`.");
    Ok(())
}

async fn test(home: &Path, project: Option<&Path>, name: &str) -> i32 {
    let sel = mcp::select(home, project);
    let Some(view) = sel
        .servers
        .iter()
        .find(|s| s.entry.config.name == name && !s.shadowed)
    else {
        eprintln!("bricks: no MCP server named '{name}'");
        return exit::FAILED;
    };
    // A repository's server runs only once the person approved it.
    if view.source == Source::Project && view.decision != Some(Decision::Approved) {
        eprintln!("bricks: '{name}' is declared by the project and not approved: `bricks mcp approve {name}` first");
        return exit::FAILED;
    }
    let manager = McpManager::connect(std::slice::from_ref(&view.entry.config)).await;
    let outcome = match manager.statuses().await.into_iter().find(|(n, _)| n == name) {
        Some((_, McpServerStatus::Connected)) => {
            let tools = manager.tools().await;
            println!("'{name}' answers: {} tool(s).", tools.len());
            for (_, tool) in tools {
                println!("  {}", tool.name);
            }
            exit::OK
        }
        Some((_, McpServerStatus::Error(e))) => {
            eprintln!("bricks: '{name}' does not answer: {e}");
            exit::FAILED
        }
        _ => {
            eprintln!("bricks: '{name}' is not connected");
            exit::FAILED
        }
    };
    manager.close().await;
    outcome
}

/// What `bricks run` says before it starts: servers waiting for approval,
/// and configuration it could not read.
pub fn notices(project: &Path) {
    let sel = mcp::select(&bricks_config::bricks_home(), Some(project));
    for s in sel.pending() {
        eprintln!(
            "bricks: the project declares the MCP server '{}' ({}); it is not connected until you \
             run `bricks mcp approve {}`",
            s.entry.config.name,
            target(&s.entry.config),
            s.entry.config.name
        );
    }
    for e in &sel.errors {
        eprintln!("bricks: {e}");
    }
}
