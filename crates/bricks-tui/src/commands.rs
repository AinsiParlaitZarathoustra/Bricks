//! The `/` commands: a declarative registry. Engine actions become
//! `Command`s of the engine contract; presentation actions (opening an
//! inspector, quitting) stay in the interface.

use cersei_agent::control::{Command, Prompt};

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
    /// `/effort`: the current model's reasoning profiles.
    EffortPicker,
    Memory,
    Context,
    /// `/context details`.
    ContextDetails,
    Cost,
    /// `/cost details`.
    CostDetails,
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
        name: "effort",
        aliases: &[],
        args: "[profile id | default]",
        description: "reasoning effort of the current model: its configured profiles; `default` = the model's own default",
        build: |arg| match arg.trim() {
            "" => present(Present::EffortPicker),
            "default" => Ok(Action::Engine(Command::SetReasoning { profile: None })),
            id => Ok(Action::Engine(Command::SetReasoning {
                profile: Some(id.to_string()),
            })),
        },
    },
    SlashCommand {
        name: "search",
        aliases: &["s"],
        args: "<text> | re:<regex>",
        description: "search the workspace's text (shared engine; never starts a language server)",
        build: |arg| {
            let arg = arg.trim();
            if arg.is_empty() {
                return Err("give a text to search for".into());
            }
            let (text, regex) = match arg.strip_prefix("re:") {
                Some(r) => (r.trim().to_string(), true),
                None => (arg.to_string(), false),
            };
            Ok(Action::Engine(Command::Search { text, regex }))
        },
    },
    SlashCommand {
        name: "agents",
        aliases: &[],
        args: "[search words] | reload",
        description:
            "sub-agent profiles: project, user and built-in (reload: read the files again)",
        build: |arg| {
            let arg = arg.trim();
            if arg == "reload" {
                return Ok(Action::Engine(Command::ReloadAgentProfiles));
            }
            // Instances: `/agents running`, `/agents status|result|cancel <id>`.
            let mut words = arg.split_whitespace();
            match (words.next(), words.next()) {
                (Some("running"), None) => {
                    return Ok(Action::Engine(Command::AgentControl {
                        action: "list".into(),
                        agent_id: None,
                        changeset_id: None,
                        job_id: None,
                    }))
                }
                (Some(a @ ("status" | "result" | "cancel")), Some(id)) => {
                    return Ok(Action::Engine(Command::AgentControl {
                        action: a.into(),
                        agent_id: Some(id.into()),
                        changeset_id: None,
                        job_id: None,
                    }))
                }
                _ => {}
            }
            Ok(Action::Engine(Command::ListAgentProfiles {
                query: arg.to_string(),
                page: 0,
            }))
        },
    },
    SlashCommand {
        name: "skills",
        aliases: &[],
        args: "[search words] | reload | trust | deny",
        description: "skills: catalog, sources, collisions, diagnostics; trust or deny this project's skills for the session",
        build: |arg| {
            let arg = arg.trim();
            Ok(Action::Engine(match arg {
                "reload" => Command::ReloadSkills,
                "trust" | "allow" => Command::TrustProjectSkills { allow: true },
                "deny" | "refuse" => Command::TrustProjectSkills { allow: false },
                query => {
                    // `/skills <words> page <n>` pages through the listing.
                    let (query, page) = match query.rsplit_once(" page ") {
                        Some((q, n)) if n.trim().parse::<usize>().is_ok() => {
                            (q.trim(), n.trim().parse().unwrap_or(0))
                        }
                        _ => match query.strip_prefix("page ") {
                            Some(n) if n.trim().parse::<usize>().is_ok() => {
                                ("", n.trim().parse().unwrap_or(0))
                            }
                            _ => (query, 0),
                        },
                    };
                    Command::ListSkills {
                        query: query.to_string(),
                        page,
                    }
                }
            }))
        },
    },
    SlashCommand {
        name: "skill",
        aliases: &[],
        args: "<name> <request>",
        description: "activate a skill, then send the request (nothing is sent if the skill cannot be activated)",
        build: |arg| {
            let arg = arg.trim();
            let (name, request) = arg.split_once(char::is_whitespace).unwrap_or((arg, ""));
            if name.is_empty() {
                return Err("give a skill name (see /skills)".into());
            }
            if request.trim().is_empty() {
                return Err(format!("give the request after the skill: /skill {name} <request>"));
            }
            Ok(Action::Engine(Command::Submit {
                prompt: Prompt::text(request.trim()).with_skills(vec![name.to_string()]),
            }))
        },
    },
    SlashCommand {
        name: "changes",
        aliases: &[],
        args: "<changeset id> inspect | apply | discard",
        description: "a sub-agent's isolated changes: show the patch, apply it to your tree (a conflict writes nothing), or discard it",
        build: |arg| {
            let mut w = arg.split_whitespace();
            let (Some(id), action) = (w.next(), w.next().unwrap_or("inspect")) else {
                return Err("give a ChangeSet id (from `changes ready`)".into());
            };
            let action = match action {
                "inspect" => "inspect_changes",
                "apply" => "apply_changes",
                "discard" => "discard_changes",
                other => return Err(format!("unknown action `{other}`: inspect, apply, discard")),
            };
            Ok(Action::Engine(Command::AgentControl {
                action: action.into(),
                agent_id: None,
                changeset_id: Some(id.into()),
                job_id: None,
            }))
        },
    },
    SlashCommand {
        name: "jobs",
        aliases: &[],
        args: "[stop <job id>]",
        description: "background jobs of the session; stop one (graceful, then forced)",
        build: |arg| {
            let mut w = arg.split_whitespace();
            match (w.next(), w.next()) {
                (None, _) => Ok(Action::Engine(Command::AgentControl {
                    action: "jobs".into(),
                    agent_id: None,
                    changeset_id: None,
                    job_id: None,
                })),
                (Some("stop"), Some(id)) => Ok(Action::Engine(Command::AgentControl {
                    action: "stop_job".into(),
                    agent_id: None,
                    changeset_id: None,
                    job_id: Some(id.into()),
                })),
                _ => Err("`/jobs` or `/jobs stop <job id>`".into()),
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
        description: "context occupation, budget and availability (`/context details`: measurement, margins, notes)",
        build: |arg| match arg.trim() {
            "" => present(Present::Context),
            "details" => present(Present::ContextDetails),
            other => Err(format!("unknown argument `{other}`: /context or /context details")),
        },
    },
    SlashCommand {
        name: "cost",
        aliases: &["usage"],
        args: "",
        description: "tokens and cost of the session (`/cost details`: per response)",
        build: |arg| match arg.trim() {
            "" => present(Present::Cost),
            "details" => present(Present::CostDetails),
            other => Err(format!("unknown argument `{other}`: /cost or /cost details")),
        },
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
        assert_eq!(
            parse("/skills").unwrap(),
            Action::Engine(Command::ListSkills {
                query: String::new(),
                page: 0
            })
        );
        assert_eq!(
            parse("/skills pdf page 2").unwrap(),
            Action::Engine(Command::ListSkills {
                query: "pdf".into(),
                page: 2
            })
        );
        assert_eq!(
            parse("/skills reload").unwrap(),
            Action::Engine(Command::ReloadSkills)
        );
        assert_eq!(
            parse("/skills trust").unwrap(),
            Action::Engine(Command::TrustProjectSkills { allow: true })
        );
        assert_eq!(
            parse("/skills deny").unwrap(),
            Action::Engine(Command::TrustProjectSkills { allow: false })
        );
        // `/skill <name> <request>`: the request is its own text.
        match parse("/skill pdf-tools extract page 3 of report.pdf").unwrap() {
            Action::Engine(Command::Submit { prompt }) => {
                assert_eq!(prompt.skills, ["pdf-tools"]);
                assert_eq!(
                    prompt,
                    Prompt::text("extract page 3 of report.pdf")
                        .with_skills(vec!["pdf-tools".into()])
                );
            }
            other => panic!("{other:?}"),
        }
        assert!(
            parse("/skill pdf-tools").is_err(),
            "no request, nothing sent"
        );
        assert_eq!(parse("/cost").unwrap(), Action::Present(Present::Cost));
        assert_eq!(
            parse("/cost details").unwrap(),
            Action::Present(Present::CostDetails)
        );
        assert_eq!(
            parse("/ctx details").unwrap(),
            Action::Present(Present::ContextDetails)
        );
        assert!(parse("/cost euros").unwrap_err().contains("/cost details"));
        assert!(parse("/context all")
            .unwrap_err()
            .contains("/context details"));
        assert!(parse("/skill").is_err());
        // Native commands keep their names; nothing is created per skill.
        assert_ne!(find("skill").unwrap().name, find("skills").unwrap().name);
        assert!(find("pdf-tools").is_none());
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
