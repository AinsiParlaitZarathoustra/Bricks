//! The shared `/` command registry over IPC (owner: Haiku 1, Sprint 14.1).
//! The registry is `cersei_agent::control::slash`, the same one the terminal
//! uses; this file only maps its actions to the contract types. Messages
//! are French.
use crate::contract::*;
use cersei_agent::control::Command;
use cersei_agent::control::slash::{self, Action, Present};

/// The registry as the desktop palette and help show it.
pub(crate) fn command_list() -> Vec<SlashCommandInfo> {
    slash::COMMANDS
        .iter()
        .map(|c| SlashCommandInfo {
            name: c.name.to_string(),
            aliases: c.aliases.iter().map(|a| a.to_string()).collect(),
            args: c.args.to_string(),
            description: c.description_fr.to_string(),
        })
        .collect()
}

#[tauri::command]
pub async fn slash_commands() -> IpcResult<Vec<SlashCommandInfo>> {
    Ok(command_list())
}

/// Parses one `/` line; `Invalid` with a French message on an unknown
/// command or a bad argument.
#[tauri::command]
pub async fn slash_parse(line: String) -> IpcResult<SlashAction> {
    parse_line(&line)
}

pub(crate) fn parse_line(line: &str) -> IpcResult<SlashAction> {
    let action = slash::parse(line).map_err(|e| IpcError {
        code: IpcErrorCode::Invalid,
        message: e.message_fr().to_string(),
    })?;
    to_ipc(action)
}

/// What a parsed action means for the desktop: an engine command in the
/// engine's own JSON (sent through `session_command`), or a presentation.
pub(crate) fn to_ipc(action: Action) -> IpcResult<SlashAction> {
    match action {
        // Resuming goes through the desktop's own open path (project,
        // folder, subscriptions), never through a controller command.
        Action::Engine(Command::Resume { session_id }) => Ok(SlashAction::Present {
            present: SlashPresent::Resume { session_id },
        }),
        Action::Engine(command) => {
            let command = serde_json::to_value(&command).map_err(|e| IpcError {
                code: IpcErrorCode::Internal,
                message: format!("commande impossible à transmettre : {e}"),
            })?;
            Ok(SlashAction::Engine { command })
        }
        Action::Present(present) => Ok(SlashAction::Present {
            present: present_to_ipc(present),
        }),
    }
}

fn present_to_ipc(present: Present) -> SlashPresent {
    match present {
        Present::ModelPicker => SlashPresent::ModelPicker,
        Present::EffortPicker => SlashPresent::EffortPicker,
        Present::Memory => SlashPresent::Memory,
        Present::Context => SlashPresent::Context,
        Present::ContextDetails => SlashPresent::ContextDetails,
        Present::Cost => SlashPresent::Cost,
        Present::CostDetails => SlashPresent::CostDetails,
        Present::Session => SlashPresent::Session,
        Present::SessionPicker => SlashPresent::SessionPicker,
        Present::Diff => SlashPresent::Diff,
        Present::Tools => SlashPresent::Tools,
        Present::Mcp => SlashPresent::Mcp,
        Present::Config => SlashPresent::Config,
        Present::Help => SlashPresent::Help,
        Present::Quit => SlashPresent::Quit,
        Present::Attach(kind, path) => SlashPresent::Attach {
            kind: kind.as_str().to_string(),
            path,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn engine(line: &str) -> serde_json::Value {
        match parse_line(line).unwrap() {
            SlashAction::Engine { command } => command,
            other => panic!("{line}: {other:?}"),
        }
    }

    fn present(line: &str) -> SlashPresent {
        match parse_line(line).unwrap() {
            SlashAction::Present { present } => present,
            other => panic!("{line}: {other:?}"),
        }
    }

    fn refusal(line: &str) -> IpcError {
        parse_line(line).unwrap_err()
    }

    #[test]
    fn engine_commands_travel_as_the_engine_json() {
        assert_eq!(
            engine("/effort default"),
            json!({"type": "set_reasoning", "profile": null})
        );
        assert_eq!(
            engine("/effort deep"),
            json!({"type": "set_reasoning", "profile": "deep"})
        );
        assert_eq!(
            engine("/model test/b deep"),
            json!({"type": "set_model", "model": "test/b", "reasoning": "deep"})
        );
        assert_eq!(
            engine("/search re:x"),
            json!({"type": "search", "text": "x", "regex": true})
        );
        assert_eq!(engine("/compact"), json!({"type": "compact"}));
        assert_eq!(
            engine("/changes cs-1 apply"),
            json!({"type": "agent_control", "action": "apply_changes", "changeset_id": "cs-1"})
        );
        assert_eq!(
            engine("/jobs stop job-7"),
            json!({"type": "agent_control", "action": "stop_job", "job_id": "job-7"})
        );
        assert_eq!(
            engine("/skill pdf extrais la page 3"),
            json!({
                "type": "submit",
                "prompt": {
                    "blocks": [{"kind": "text", "text": "extrais la page 3"}],
                    "skills": ["pdf"]
                }
            })
        );
    }

    #[test]
    fn engine_json_is_a_command_the_engine_reads_back() {
        for line in [
            "/effort default",
            "/search re:x",
            "/changes cs-1 discard",
            "/jobs",
        ] {
            let value = engine(line);
            let back: Command = serde_json::from_value(value.clone())
                .unwrap_or_else(|e| panic!("{line}: {e} in {value}"));
            let original = slash::parse(line).unwrap();
            assert_eq!(original, Action::Engine(back), "{line}");
        }
    }

    #[test]
    fn resume_with_an_id_opens_through_the_desktop() {
        assert!(matches!(
            present("/resume 2026-x"),
            SlashPresent::Resume { ref session_id } if session_id == "2026-x"
        ));
        assert!(matches!(present("/sessions"), SlashPresent::SessionPicker));
        assert!(matches!(present("/resume"), SlashPresent::SessionPicker));
    }

    #[test]
    fn presentations_map_one_to_one() {
        assert!(matches!(present("/model"), SlashPresent::ModelPicker));
        assert!(matches!(present("/effort"), SlashPresent::EffortPicker));
        assert!(matches!(present("/memory"), SlashPresent::Memory));
        assert!(matches!(present("/ctx"), SlashPresent::Context));
        assert!(matches!(
            present("/context details"),
            SlashPresent::ContextDetails
        ));
        assert!(matches!(present("/usage"), SlashPresent::Cost));
        assert!(matches!(
            present("/cost details"),
            SlashPresent::CostDetails
        ));
        assert!(matches!(present("/session"), SlashPresent::Session));
        assert!(matches!(present("/diff"), SlashPresent::Diff));
        assert!(matches!(present("/tools"), SlashPresent::Tools));
        assert!(matches!(present("/mcp"), SlashPresent::Mcp));
        assert!(matches!(present("/config"), SlashPresent::Config));
        assert!(matches!(present("/?"), SlashPresent::Help));
        assert!(matches!(present("/q"), SlashPresent::Quit));
    }

    #[test]
    fn attachments_carry_their_kind_and_path_as_typed() {
        assert!(matches!(
            present("/file \"Mes documents/rapport final é.md\""),
            SlashPresent::Attach { ref kind, ref path }
                if kind == "file" && path == "Mes documents/rapport final é.md"
        ));
        assert!(matches!(
            present("/folder Projets/2026"),
            SlashPresent::Attach { ref kind, ref path }
                if kind == "folder" && path == "Projets/2026"
        ));
        assert!(matches!(
            present("/img \"capture écran.png\""),
            SlashPresent::Attach { ref kind, ref path }
                if kind == "image" && path == "capture écran.png"
        ));
    }

    #[test]
    fn unknown_commands_and_bad_arguments_are_invalid_in_french() {
        let unknown = refusal("/nope");
        assert_eq!(unknown.code, IpcErrorCode::Invalid);
        assert_eq!(unknown.message, "commande inconnue /nope (essayez /help)");

        let no_request = refusal("/skill pdf");
        assert_eq!(no_request.code, IpcErrorCode::Invalid);
        assert!(no_request.message.contains("donnez la demande"));

        let bad_arg = refusal("/context all");
        assert_eq!(bad_arg.code, IpcErrorCode::Invalid);
        assert_eq!(
            bad_arg.message,
            "argument inconnu `all` : /context ou /context details"
        );

        assert_eq!(refusal("/file").code, IpcErrorCode::Invalid);
        assert_eq!(refusal("model").code, IpcErrorCode::Invalid);
    }

    #[test]
    fn the_palette_lists_the_registry_in_french() {
        let list = command_list();
        assert_eq!(list.len(), slash::COMMANDS.len());
        let model = list.iter().find(|c| c.name == "model").unwrap();
        assert_eq!(model.aliases, vec!["m".to_string()]);
        assert_eq!(model.args, "[provider/model [profile]]");
        assert!(
            model.description.contains("modèle"),
            "{}",
            model.description
        );
        let mut names = std::collections::HashSet::new();
        for c in &list {
            assert!(names.insert(c.name.clone()), "/{} twice", c.name);
            assert!(!c.description.is_empty(), "/{}", c.name);
        }
        assert!(list.iter().any(|c| c.name == "changes"));
        assert!(list.iter().any(|c| c.name == "jobs"));
    }
}
