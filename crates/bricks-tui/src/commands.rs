//! The `/` commands: a declarative registry. Engine actions become
//! `Command`s of the engine contract; presentation actions (opening an
//! inspector, quitting) stay in the interface.

use cersei_agent::control::Command;

/// What a slash command does.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Sent to the engine.
    Engine(Command),
    /// Handled by the interface.
    Present(Present),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Present {
    ModelPicker,
    Memory,
    Context,
    Cost,
    Session,
    SessionPicker,
    Diff,
    Tools,
    Mcp,
    Config,
    Help,
    Quit,
    /// `/file`, `/folder`, `/image` with their argument.
    Attach(AttachKind, String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachKind {
    File,
    Folder,
    Image,
}

pub struct SlashCommand {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub args: &'static str,
    pub description: &'static str,
    build: fn(&str) -> Result<Action, String>,
}

fn present(p: Present) -> Result<Action, String> {
    Ok(Action::Present(p))
}

fn attach(kind: AttachKind, arg: &str) -> Result<Action, String> {
    let path = arg.trim().trim_matches('"');
    if path.is_empty() {
        return Err("give a path".into());
    }
    Ok(Action::Present(Present::Attach(kind, path.to_string())))
}

pub const COMMANDS: &[SlashCommand] = &[
    SlashCommand {
        name: "model",
        aliases: &["m"],
        args: "[provider/model [profile]]",
        description: "choose the model and reasoning profile (from the configuration)",
        build: |arg| {
            let mut parts = arg.split_whitespace();
            match parts.next() {
                None => present(Present::ModelPicker),
                Some(m) => Ok(Action::Engine(Command::SetModel {
                    model: Some(m.to_string()),
                    reasoning: parts.next().map(str::to_string),
                })),
            }
        },
    },
    SlashCommand {
        name: "memory",
        aliases: &[],
        args: "",
        description: "long-term memory: space, last recall, maintenance",
        build: |_| present(Present::Memory),
    },
    SlashCommand {
        name: "context",
        aliases: &["ctx"],
        args: "",
        description: "context occupation, window and provenance of the numbers",
        build: |_| present(Present::Context),
    },
    SlashCommand {
        name: "cost",
        aliases: &["usage"],
        args: "",
        description: "tokens and cost of the session (unknown without prices)",
        build: |_| present(Present::Cost),
    },
    SlashCommand {
        name: "session",
        aliases: &[],
        args: "",
        description: "this session: id, directory, model",
        build: |_| present(Present::Session),
    },
    SlashCommand {
        name: "resume",
        aliases: &["sessions"],
        args: "[session id]",
        description: "switch to a stored session",
        build: |arg| {
            let id = arg.trim();
            if id.is_empty() {
                present(Present::SessionPicker)
            } else {
                Ok(Action::Engine(Command::Resume {
                    session_id: id.to_string(),
                }))
            }
        },
    },
    SlashCommand {
        name: "compact",
        aliases: &[],
        args: "",
        description: "summarise the context now",
        build: |_| Ok(Action::Engine(Command::Compact)),
    },
    SlashCommand {
        name: "clear",
        aliases: &[],
        args: "",
        description: "empty the active context (kept in the session's raw history)",
        build: |_| Ok(Action::Engine(Command::ClearContext)),
    },
    SlashCommand {
        name: "diff",
        aliases: &[],
        args: "",
        description: "proposed change, changes applied in this session, git diff",
        build: |_| present(Present::Diff),
    },
    SlashCommand {
        name: "tools",
        aliases: &[],
        args: "",
        description: "tools available to the agent and their permission level",
        build: |_| present(Present::Tools),
    },
    SlashCommand {
        name: "mcp",
        aliases: &[],
        args: "",
        description: "MCP servers and their state",
        build: |_| present(Present::Mcp),
    },
    SlashCommand {
        name: "config",
        aliases: &[],
        args: "",
        description: "effective configuration (no secrets)",
        build: |_| present(Present::Config),
    },
    SlashCommand {
        name: "file",
        aliases: &[],
        args: "<path>",
        description: "attach a file (captured when sent)",
        build: |arg| attach(AttachKind::File, arg),
    },
    SlashCommand {
        name: "folder",
        aliases: &[],
        args: "<path>",
        description: "attach a folder listing (bounded)",
        build: |arg| attach(AttachKind::Folder, arg),
    },
    SlashCommand {
        name: "image",
        aliases: &["img"],
        args: "<path>",
        description: "attach an image (png, jpeg, gif, webp)",
        build: |arg| attach(AttachKind::Image, arg),
    },
    SlashCommand {
        name: "help",
        aliases: &["?"],
        args: "",
        description: "commands and keys",
        build: |_| present(Present::Help),
    },
    SlashCommand {
        name: "quit",
        aliases: &["exit", "q"],
        args: "",
        description: "leave (the session stays stored)",
        build: |_| present(Present::Quit),
    },
];

pub fn find(name: &str) -> Option<&'static SlashCommand> {
    COMMANDS
        .iter()
        .find(|c| c.name == name || c.aliases.contains(&name))
}

/// Parse a full `/command args` line.
pub fn parse(line: &str) -> Result<Action, String> {
    let line = line.trim();
    let rest = line.strip_prefix('/').ok_or("not a command")?;
    let (name, arg) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    let cmd = find(name).ok_or_else(|| format!("unknown command /{name} (try /help)"))?;
    (cmd.build)(arg)
}

/// Commands whose name or alias starts with `prefix`, in registry order.
pub fn complete(prefix: &str) -> Vec<&'static SlashCommand> {
    COMMANDS
        .iter()
        .filter(|c| c.name.starts_with(prefix) || c.aliases.iter().any(|a| a.starts_with(prefix)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_parses_and_completes() {
        assert_eq!(
            parse("/model").unwrap(),
            Action::Present(Present::ModelPicker)
        );
        assert_eq!(
            parse("/m test/b deep").unwrap(),
            Action::Engine(Command::SetModel {
                model: Some("test/b".into()),
                reasoning: Some("deep".into())
            })
        );
        assert_eq!(parse("/compact").unwrap(), Action::Engine(Command::Compact));
        assert_eq!(
            parse("/resume 2026-x").unwrap(),
            Action::Engine(Command::Resume {
                session_id: "2026-x".into()
            })
        );
        assert_eq!(
            parse("/image \"capture écran.png\"").unwrap(),
            Action::Present(Present::Attach(
                AttachKind::Image,
                "capture écran.png".into()
            ))
        );
        assert!(parse("/file").is_err());
        assert!(parse("/nope").unwrap_err().contains("/help"));
        assert_eq!(
            complete("co").iter().map(|c| c.name).collect::<Vec<_>>(),
            vec!["context", "cost", "compact", "config"]
        );
        for required in [
            "model", "memory", "context", "cost", "session", "resume", "compact", "diff", "tools",
            "mcp", "config", "help", "quit",
        ] {
            assert!(find(required).is_some(), "/{required}");
        }
    }
}
