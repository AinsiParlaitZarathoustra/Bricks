//! The `/` command registry, one list for every interface.
//!
//! A line such as `/model deep` becomes an [`Action`]: a [`Command`] sent to
//! the engine, or a presentation the interface performs itself (an
//! inspector, a picker, an attachment). The registry holds the names, the
//! aliases, the argument syntax, the descriptions (English for the terminal
//! help, French for the desktop) and the parse errors (both languages). It
//! has no terminal or window dependency: each frontend maps [`Present`] to
//! its own views.

use super::protocol::{Command, Prompt};

/// What a slash command does.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Sent to the engine.
    Engine(Command),
    /// Handled by the interface.
    Present(Present),
}

/// What the interface does itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Present {
    /// `/model` without arguments: the model picker.
    ModelPicker,
    /// `/effort`: the current model's reasoning profiles.
    EffortPicker,
    /// `/memory`: the current project's memory.
    Memory,
    /// `/memory global`: the memory shared by every project.
    MemoryGlobal,
    Context,
    /// `/context details`.
    ContextDetails,
    Cost,
    /// `/cost details`.
    CostDetails,
    Session,
    /// `/resume` without an id: the session picker.
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

impl AttachKind {
    /// `file`, `folder` or `image`.
    pub fn as_str(self) -> &'static str {
        match self {
            AttachKind::File => "file",
            AttachKind::Folder => "folder",
            AttachKind::Image => "image",
        }
    }
}

/// A line the registry refuses, with the reason in English and in French.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashError {
    message: String,
    message_fr: String,
}

impl SlashError {
    fn new(message: impl Into<String>, message_fr: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            message_fr: message_fr.into(),
        }
    }

    /// The reason in English (terminal interface).
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The reason in French (desktop).
    pub fn message_fr(&self) -> &str {
        &self.message_fr
    }
}

impl std::fmt::Display for SlashError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SlashError {}

pub struct SlashCommand {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    /// Argument syntax (the same in every language).
    pub args: &'static str,
    /// English: the terminal help and palette.
    pub description: &'static str,
    /// French: the desktop palette and help.
    pub description_fr: &'static str,
    build: fn(&str) -> Result<Action, SlashError>,
}

fn present(p: Present) -> Result<Action, SlashError> {
    Ok(Action::Present(p))
}

fn attach(kind: AttachKind, arg: &str) -> Result<Action, SlashError> {
    let path = arg.trim().trim_matches('"');
    if path.is_empty() {
        return Err(SlashError::new("give a path", "donnez un chemin"));
    }
    Ok(Action::Present(Present::Attach(kind, path.to_string())))
}

pub const COMMANDS: &[SlashCommand] = &[
    SlashCommand {
        name: "model",
        aliases: &["m"],
        args: "[provider/model [profile]]",
        description: "choose the model and reasoning profile (from the configuration)",
        description_fr: "choisir le modèle et le profil de raisonnement (selon la configuration)",
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
        description_fr: "effort de raisonnement du modèle courant : ses profils configurés ; `default` = le défaut du modèle",
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
        description_fr: "rechercher du texte dans le projet (moteur partagé ; ne lance jamais de serveur de langage)",
        build: |arg| {
            let arg = arg.trim();
            if arg.is_empty() {
                return Err(SlashError::new(
                    "give a text to search for",
                    "donnez un texte à rechercher",
                ));
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
        description: "sub-agent profiles: project, user and built-in (reload: read the files again)",
        description_fr: "profils de sous-agents : projet, utilisateur et intégrés (reload : relire les fichiers)",
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
                        message: None,
                    }));
                }
                (Some(a @ ("status" | "result" | "cancel")), Some(id)) => {
                    return Ok(Action::Engine(Command::AgentControl {
                        action: a.into(),
                        agent_id: Some(id.into()),
                        changeset_id: None,
                        job_id: None,
                        message: None,
                    }));
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
        description_fr: "compétences : catalogue, sources, conflits, diagnostics ; faire confiance aux compétences de ce projet, ou les refuser, pour la session",
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
        description_fr: "activer une compétence, puis envoyer la demande (rien n'est envoyé si la compétence ne peut pas être activée)",
        build: |arg| {
            let arg = arg.trim();
            let (name, request) = arg.split_once(char::is_whitespace).unwrap_or((arg, ""));
            if name.is_empty() {
                return Err(SlashError::new(
                    "give a skill name (see /skills)",
                    "donnez le nom d'une compétence (voir /skills)",
                ));
            }
            if request.trim().is_empty() {
                return Err(SlashError::new(
                    format!("give the request after the skill: /skill {name} <request>"),
                    format!("donnez la demande après la compétence : /skill {name} <demande>"),
                ));
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
        description_fr: "les modifications isolées d'un sous-agent : afficher le correctif, l'appliquer à votre arborescence (un conflit n'écrit rien) ou l'écarter",
        build: |arg| {
            let mut w = arg.split_whitespace();
            let (Some(id), action) = (w.next(), w.next().unwrap_or("inspect")) else {
                return Err(SlashError::new(
                    "give a ChangeSet id (from `changes ready`)",
                    "donnez l'identifiant d'un ChangeSet (voir `changes ready`)",
                ));
            };
            let action = match action {
                "inspect" => "inspect_changes",
                "apply" => "apply_changes",
                "discard" => "discard_changes",
                other => {
                    return Err(SlashError::new(
                        format!("unknown action `{other}`: inspect, apply, discard"),
                        format!("action inconnue `{other}` : inspect, apply, discard"),
                    ));
                }
            };
            Ok(Action::Engine(Command::AgentControl {
                action: action.into(),
                agent_id: None,
                changeset_id: Some(id.into()),
                job_id: None,
                message: None,
            }))
        },
    },
    SlashCommand {
        name: "jobs",
        aliases: &[],
        args: "[stop <job id>]",
        description: "background jobs of the session; stop one (graceful, then forced)",
        description_fr: "tâches d'arrière-plan de la session ; arrêter l'une d'elles (arrêt progressif, puis forcé)",
        build: |arg| {
            let mut w = arg.split_whitespace();
            match (w.next(), w.next()) {
                (None, _) => Ok(Action::Engine(Command::AgentControl {
                    action: "jobs".into(),
                    agent_id: None,
                    changeset_id: None,
                    job_id: None,
                    message: None,
                })),
                (Some("stop"), Some(id)) => Ok(Action::Engine(Command::AgentControl {
                    action: "stop_job".into(),
                    agent_id: None,
                    changeset_id: None,
                    job_id: Some(id.into()),
                    message: None,
                })),
                _ => Err(SlashError::new(
                    "`/jobs` or `/jobs stop <job id>`",
                    "`/jobs` ou `/jobs stop <identifiant de tâche>`",
                )),
            }
        },
    },
    SlashCommand {
        name: "memory",
        aliases: &[],
        args: "[global]",
        description: "long-term memory: the project's, or the global one",
        description_fr: "mémoire à long terme : celle du projet, ou la mémoire globale",
        build: |arg| match arg.trim() {
            "" => present(Present::Memory),
            "global" => present(Present::MemoryGlobal),
            _ => Err(SlashError::new(
                "`/memory` or `/memory global`",
                "`/memory` ou `/memory global`",
            )),
        },
    },
    SlashCommand {
        name: "context",
        aliases: &["ctx"],
        args: "",
        description: "context occupation, budget and availability (`/context details`: measurement, margins, notes)",
        description_fr: "occupation du contexte, budget et disponibilité (`/context details` : mesure, marges, notes)",
        build: |arg| match arg.trim() {
            "" => present(Present::Context),
            "details" => present(Present::ContextDetails),
            other => Err(SlashError::new(
                format!("unknown argument `{other}`: /context or /context details"),
                format!("argument inconnu `{other}` : /context ou /context details"),
            )),
        },
    },
    SlashCommand {
        name: "cost",
        aliases: &["usage"],
        args: "",
        description: "tokens and cost of the session (`/cost details`: per response)",
        description_fr: "jetons et coût de la session (`/cost details` : par réponse)",
        build: |arg| match arg.trim() {
            "" => present(Present::Cost),
            "details" => present(Present::CostDetails),
            other => Err(SlashError::new(
                format!("unknown argument `{other}`: /cost or /cost details"),
                format!("argument inconnu `{other}` : /cost ou /cost details"),
            )),
        },
    },
    SlashCommand {
        name: "session",
        aliases: &[],
        args: "",
        description: "this session: id, directory, model",
        description_fr: "cette session : identifiant, dossier, modèle",
        build: |_| present(Present::Session),
    },
    SlashCommand {
        name: "resume",
        aliases: &["sessions"],
        args: "[session id]",
        description: "switch to a stored session",
        description_fr: "reprendre une session enregistrée",
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
        description_fr: "résumer le contexte maintenant",
        build: |_| Ok(Action::Engine(Command::Compact)),
    },
    SlashCommand {
        name: "clear",
        aliases: &[],
        args: "",
        description: "empty the active context (kept in the session's raw history)",
        description_fr: "vider le contexte actif (il reste dans l'historique brut de la session)",
        build: |_| Ok(Action::Engine(Command::ClearContext)),
    },
    SlashCommand {
        name: "diff",
        aliases: &[],
        args: "",
        description: "proposed change, changes applied in this session, git diff",
        description_fr: "modifications proposées, modifications appliquées dans cette session, diff git",
        build: |_| present(Present::Diff),
    },
    SlashCommand {
        name: "tools",
        aliases: &[],
        args: "",
        description: "tools available to the agent and their permission level",
        description_fr: "outils disponibles pour l'agent et leur niveau de permission",
        build: |_| present(Present::Tools),
    },
    SlashCommand {
        name: "mcp",
        aliases: &[],
        args: "",
        description: "MCP servers and their state",
        description_fr: "serveurs MCP et leur état",
        build: |_| present(Present::Mcp),
    },
    SlashCommand {
        name: "config",
        aliases: &[],
        args: "",
        description: "effective configuration (no secrets)",
        description_fr: "configuration effective (sans secrets)",
        build: |_| present(Present::Config),
    },
    SlashCommand {
        name: "file",
        aliases: &[],
        args: "<path>",
        description: "attach a file (captured when sent)",
        description_fr: "joindre un fichier (capturé à l'envoi)",
        build: |arg| attach(AttachKind::File, arg),
    },
    SlashCommand {
        name: "folder",
        aliases: &[],
        args: "<path>",
        description: "attach a folder listing (bounded)",
        description_fr: "joindre la liste d'un dossier (bornée)",
        build: |arg| attach(AttachKind::Folder, arg),
    },
    SlashCommand {
        name: "image",
        aliases: &["img"],
        args: "<path>",
        description: "attach an image (png, jpeg, gif, webp)",
        description_fr: "joindre une image (png, jpeg, gif, webp)",
        build: |arg| attach(AttachKind::Image, arg),
    },
    SlashCommand {
        name: "help",
        aliases: &["?"],
        args: "",
        description: "commands and keys",
        description_fr: "commandes et raccourcis",
        build: |_| present(Present::Help),
    },
    SlashCommand {
        name: "quit",
        aliases: &["exit", "q"],
        args: "",
        description: "leave (the session stays stored)",
        description_fr: "quitter (la session reste enregistrée)",
        build: |_| present(Present::Quit),
    },
];

/// The command with this name or alias.
pub fn find(name: &str) -> Option<&'static SlashCommand> {
    COMMANDS
        .iter()
        .find(|c| c.name == name || c.aliases.contains(&name))
}

/// Parse a full `/command args` line.
pub fn parse(line: &str) -> Result<Action, SlashError> {
    let line = line.trim();
    let Some(rest) = line.strip_prefix('/') else {
        return Err(SlashError::new(
            "not a command",
            "ce n'est pas une commande",
        ));
    };
    let (name, arg) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    let Some(cmd) = find(name) else {
        return Err(SlashError::new(
            format!("unknown command /{name} (try /help)"),
            format!("commande inconnue /{name} (essayez /help)"),
        ));
    };
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
    use std::collections::HashSet;

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
        assert!(
            parse("/cost euros")
                .unwrap_err()
                .message()
                .contains("/cost details")
        );
        assert!(
            parse("/context all")
                .unwrap_err()
                .message()
                .contains("/context details")
        );
        assert!(parse("/skill").is_err());
        // Native commands keep their names; nothing is created per skill.
        assert_ne!(find("skill").unwrap().name, find("skills").unwrap().name);
        assert!(find("pdf-tools").is_none());
        assert!(parse("/nope").unwrap_err().message().contains("/help"));
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

    #[test]
    fn every_name_and_alias_resolves_to_its_own_command() {
        // `COMMANDS` is a const: its addresses differ per use, so the
        // identity is the name.
        let mut seen = HashSet::new();
        for c in COMMANDS {
            assert_eq!(find(c.name).unwrap().name, c.name, "/{}", c.name);
            assert!(seen.insert(c.name), "/{} is declared twice", c.name);
            for a in c.aliases {
                assert_eq!(find(a).unwrap().name, c.name, "alias /{a}");
                assert!(seen.insert(*a), "alias /{a} is declared twice");
            }
        }
        // The aliases the terminal and the CLI documented.
        for (alias, want) in [
            ("m", "model"),
            ("s", "search"),
            ("ctx", "context"),
            ("usage", "cost"),
            ("sessions", "resume"),
            ("img", "image"),
            ("exit", "quit"),
            ("q", "quit"),
            ("?", "help"),
        ] {
            assert_eq!(find(alias).unwrap().name, want, "/{alias}");
        }
    }

    #[test]
    fn every_command_has_a_description_in_both_languages() {
        for c in COMMANDS {
            assert!(!c.description.is_empty(), "/{}", c.name);
            assert!(!c.description_fr.is_empty(), "/{}", c.name);
        }
    }

    #[test]
    fn aliases_act_like_their_command() {
        assert_eq!(parse("/m").unwrap(), Action::Present(Present::ModelPicker));
        assert!(matches!(
            parse("/s needle").unwrap(),
            Action::Engine(Command::Search { .. })
        ));
        assert_eq!(parse("/ctx").unwrap(), Action::Present(Present::Context));
        assert_eq!(parse("/usage").unwrap(), Action::Present(Present::Cost));
        assert_eq!(
            parse("/sessions").unwrap(),
            Action::Present(Present::SessionPicker)
        );
        assert_eq!(
            parse("/img a.png").unwrap(),
            Action::Present(Present::Attach(AttachKind::Image, "a.png".into()))
        );
        assert_eq!(parse("/exit").unwrap(), Action::Present(Present::Quit));
        assert_eq!(parse("/q").unwrap(), Action::Present(Present::Quit));
        assert_eq!(parse("/?").unwrap(), Action::Present(Present::Help));
    }

    #[test]
    fn effort_default_resets_the_profile() {
        assert_eq!(
            parse("/effort default").unwrap(),
            Action::Engine(Command::SetReasoning { profile: None })
        );
        assert_eq!(
            parse("/effort deep").unwrap(),
            Action::Engine(Command::SetReasoning {
                profile: Some("deep".into())
            })
        );
        assert_eq!(
            parse("/effort").unwrap(),
            Action::Present(Present::EffortPicker)
        );
    }

    #[test]
    fn search_takes_a_text_or_a_regex() {
        assert_eq!(
            parse("/search re:x").unwrap(),
            Action::Engine(Command::Search {
                text: "x".into(),
                regex: true
            })
        );
        assert_eq!(
            parse("/s needle here").unwrap(),
            Action::Engine(Command::Search {
                text: "needle here".into(),
                regex: false
            })
        );
        assert_eq!(
            parse("/search re:  a+b ").unwrap(),
            Action::Engine(Command::Search {
                text: "a+b".into(),
                regex: true
            })
        );
        assert!(parse("/search").is_err());
        assert!(parse("/search   ").is_err());
    }

    #[test]
    fn changes_act_on_a_changeset() {
        assert_eq!(
            parse("/changes cs-1 apply").unwrap(),
            Action::Engine(Command::AgentControl {
                action: "apply_changes".into(),
                agent_id: None,
                changeset_id: Some("cs-1".into()),
                job_id: None,
                message: None,
            })
        );
        assert_eq!(
            parse("/changes cs-1").unwrap(),
            Action::Engine(Command::AgentControl {
                action: "inspect_changes".into(),
                agent_id: None,
                changeset_id: Some("cs-1".into()),
                job_id: None,
                message: None,
            })
        );
        assert_eq!(
            parse("/changes cs-1 discard").unwrap(),
            Action::Engine(Command::AgentControl {
                action: "discard_changes".into(),
                agent_id: None,
                changeset_id: Some("cs-1".into()),
                job_id: None,
                message: None,
            })
        );
        assert!(parse("/changes").is_err());
        assert!(
            parse("/changes cs-1 merge")
                .unwrap_err()
                .message()
                .contains("inspect, apply, discard")
        );
    }

    #[test]
    fn jobs_list_and_stop() {
        assert_eq!(
            parse("/jobs").unwrap(),
            Action::Engine(Command::AgentControl {
                action: "jobs".into(),
                agent_id: None,
                changeset_id: None,
                job_id: None,
                message: None,
            })
        );
        assert_eq!(
            parse("/jobs stop job-7").unwrap(),
            Action::Engine(Command::AgentControl {
                action: "stop_job".into(),
                agent_id: None,
                changeset_id: None,
                job_id: Some("job-7".into()),
                message: None,
            })
        );
        assert!(parse("/jobs stop").is_err());
        assert!(parse("/jobs kill job-7").is_err());
    }

    #[test]
    fn unknown_commands_and_bad_arguments_are_errors() {
        assert!(parse("/nope").is_err());
        assert!(parse("/").is_err());
        assert!(
            parse("model")
                .unwrap_err()
                .message()
                .contains("not a command")
        );
        assert!(parse("/context all").is_err());
        assert!(parse("/cost euros").is_err());
        assert!(parse("/skill").is_err());
        assert!(parse("/skill pdf-tools   ").is_err());
        assert!(
            parse("/agents").is_ok(),
            "agents without arguments lists the profiles"
        );
    }

    #[test]
    fn errors_are_also_given_in_french() {
        let cases = [
            ("/nope", "commande inconnue /nope (essayez /help)"),
            ("model", "ce n'est pas une commande"),
            ("/skill", "donnez le nom d'une compétence (voir /skills)"),
            (
                "/skill pdf-tools",
                "donnez la demande après la compétence : /skill pdf-tools <demande>",
            ),
            ("/file", "donnez un chemin"),
            ("/search", "donnez un texte à rechercher"),
            (
                "/cost euros",
                "argument inconnu `euros` : /cost ou /cost details",
            ),
            (
                "/context all",
                "argument inconnu `all` : /context ou /context details",
            ),
            (
                "/changes",
                "donnez l'identifiant d'un ChangeSet (voir `changes ready`)",
            ),
            (
                "/jobs kill x",
                "`/jobs` ou `/jobs stop <identifiant de tâche>`",
            ),
        ];
        for (line, want) in cases {
            let err = parse(line).unwrap_err();
            assert_eq!(err.message_fr(), want, "{line}");
            assert!(!err.message().is_empty(), "{line}");
            assert_ne!(err.message(), err.message_fr(), "{line} is translated");
        }
    }

    #[test]
    fn attachment_paths_keep_spaces_unicode_and_quotes_out() {
        assert_eq!(
            parse("/file my notes.txt").unwrap(),
            Action::Present(Present::Attach(AttachKind::File, "my notes.txt".into()))
        );
        assert_eq!(
            parse("/file \"Mes documents/rapport final é.md\"").unwrap(),
            Action::Present(Present::Attach(
                AttachKind::File,
                "Mes documents/rapport final é.md".into()
            ))
        );
        assert_eq!(
            parse("/folder \"Projets/2026 — données\"").unwrap(),
            Action::Present(Present::Attach(
                AttachKind::Folder,
                "Projets/2026 — données".into()
            ))
        );
        assert_eq!(
            parse("/image \"capture écran.png\"").unwrap(),
            Action::Present(Present::Attach(
                AttachKind::Image,
                "capture écran.png".into()
            ))
        );
        assert_eq!(
            parse("/file   notes.md  ").unwrap(),
            Action::Present(Present::Attach(AttachKind::File, "notes.md".into()))
        );
        assert!(parse("/file \"\"").is_err(), "quotes only: no path");
        assert!(parse("/file    ").is_err());
    }

    #[test]
    fn attach_kinds_have_their_wire_names() {
        assert_eq!(AttachKind::File.as_str(), "file");
        assert_eq!(AttachKind::Folder.as_str(), "folder");
        assert_eq!(AttachKind::Image.as_str(), "image");
    }
}
