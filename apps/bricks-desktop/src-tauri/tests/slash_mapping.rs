//! Correspondence matrix for the `/` commands.
//!
//! The registry is `cersei_agent::control::slash::COMMANDS`; the desktop reaches it
//! only through the public `slash_parse` / `slash_commands` commands. Each row
//! below gives the expected wire value (`SlashAction` as the frontend receives it)
//! for one line, following `apps/bricks-desktop/docs/tui-inventory.md`.
//!
//! Run: `cargo test -p bricks-desktop --test slash_mapping` (from the repo root).

use std::collections::BTreeSet;

use bricks_desktop_lib::commands::slash::{slash_commands, slash_parse};
use bricks_desktop_lib::contract::{IpcError, IpcErrorCode};
use cersei_agent::control::Command;
use cersei_agent::control::slash::COMMANDS;
use serde_json::{Value, json};

/// What one line must produce.
#[derive(Debug)]
enum Want {
    /// `kind: engine` carrying exactly this engine command (`Command` JSON).
    Engine(Value),
    /// `kind: present` carrying exactly this view.
    Present(Value),
    /// `Invalid` with exactly this French message.
    Invalid(&'static str),
}

struct Case {
    /// The registry name or alias the line starts with (`model`, `m`, `?`...).
    token: &'static str,
    line: &'static str,
    want: Want,
}

fn engine(command: Value) -> Value {
    json!({ "kind": "engine", "command": command })
}

fn present(view: Value) -> Value {
    json!({ "kind": "present", "present": view })
}

fn case(token: &'static str, line: &'static str, want: Want) -> Case {
    Case { token, line, want }
}

fn matrix() -> Vec<Case> {
    use Want::{Engine, Invalid, Present as Pres};
    vec![
        // /model (/m)
        case("model", "/model", Pres(json!({"view": "model_picker"}))),
        case("m", "/m", Pres(json!({"view": "model_picker"}))),
        case(
            "model",
            "/model test/b deep",
            Engine(json!({"type": "set_model", "model": "test/b", "reasoning": "deep"})),
        ),
        case("model", "/model test/b", Engine(json!({"type": "set_model", "model": "test/b"}))),
        case("m", "/m test/b", Engine(json!({"type": "set_model", "model": "test/b"}))),
        // /effort: `default` resets the profile, an id is sent as is (the engine refuses unknown ids)
        case("effort", "/effort", Pres(json!({"view": "effort_picker"}))),
        case("effort", "/effort default", Engine(json!({"type": "set_reasoning", "profile": null}))),
        case("effort", "/effort   default  ", Engine(json!({"type": "set_reasoning", "profile": null}))),
        case("effort", "/effort deep", Engine(json!({"type": "set_reasoning", "profile": "deep"}))),
        case("effort", "/effort inconnu", Engine(json!({"type": "set_reasoning", "profile": "inconnu"}))),
        // /search (/s): plain text, `re:` regex
        case(
            "search",
            "/search rapport final",
            Engine(json!({"type": "search", "text": "rapport final", "regex": false})),
        ),
        case("s", "/s rapport", Engine(json!({"type": "search", "text": "rapport", "regex": false}))),
        case("search", "/search re:a.b", Engine(json!({"type": "search", "text": "a.b", "regex": true}))),
        case(
            "s",
            "/s re:^fn\\s+main",
            Engine(json!({"type": "search", "text": "^fn\\s+main", "regex": true})),
        ),
        // Quirk kept from the TUI: an empty regex is passed to the engine.
        case("search", "/search re:", Engine(json!({"type": "search", "text": "", "regex": true}))),
        case("search", "/search", Invalid("donnez un texte à rechercher")),
        case("s", "/s   ", Invalid("donnez un texte à rechercher")),
        // /agents: profiles, then the instance sub-commands
        case(
            "agents",
            "/agents",
            Engine(json!({"type": "list_agent_profiles", "query": "", "page": 0})),
        ),
        case(
            "agents",
            "/agents explore rust",
            Engine(json!({"type": "list_agent_profiles", "query": "explore rust", "page": 0})),
        ),
        case("agents", "/agents reload", Engine(json!({"type": "reload_agent_profiles"}))),
        case("agents", "/agents running", Engine(json!({"type": "agent_control", "action": "list"}))),
        case(
            "agents",
            "/agents status a1",
            Engine(json!({"type": "agent_control", "action": "status", "agent_id": "a1"})),
        ),
        case(
            "agents",
            "/agents result a1",
            Engine(json!({"type": "agent_control", "action": "result", "agent_id": "a1"})),
        ),
        case(
            "agents",
            "/agents cancel a1",
            Engine(json!({"type": "agent_control", "action": "cancel", "agent_id": "a1"})),
        ),
        // Quirk kept from the TUI: `/agents status` without an id searches profiles for the word.
        case(
            "agents",
            "/agents status",
            Engine(json!({"type": "list_agent_profiles", "query": "status", "page": 0})),
        ),
        // /skills: catalogue, paging, trust decisions
        case("skills", "/skills", Engine(json!({"type": "list_skills", "query": "", "page": 0}))),
        case(
            "skills",
            "/skills pdf page 2",
            Engine(json!({"type": "list_skills", "query": "pdf", "page": 2})),
        ),
        case("skills", "/skills page 3", Engine(json!({"type": "list_skills", "query": "", "page": 3}))),
        case("skills", "/skills reload", Engine(json!({"type": "reload_skills"}))),
        case("skills", "/skills trust", Engine(json!({"type": "trust_project_skills", "allow": true}))),
        case("skills", "/skills allow", Engine(json!({"type": "trust_project_skills", "allow": true}))),
        case("skills", "/skills deny", Engine(json!({"type": "trust_project_skills", "allow": false}))),
        case("skills", "/skills refuse", Engine(json!({"type": "trust_project_skills", "allow": false}))),
        // /skill <name> <request>: the skill travels in `prompt.skills`
        case(
            "skill",
            "/skill pdf extrais la page 3",
            Engine(json!({
                "type": "submit",
                "prompt": {
                    "blocks": [{"kind": "text", "text": "extrais la page 3"}],
                    "skills": ["pdf"]
                }
            })),
        ),
        case(
            "skill",
            "/skill pdf",
            Invalid("donnez la demande après la compétence : /skill pdf <demande>"),
        ),
        case("skill", "/skill", Invalid("donnez le nom d'une compétence (voir /skills)")),
        // /changes <id> [inspect|apply|discard]
        case(
            "changes",
            "/changes cs-1",
            Engine(json!({"type": "agent_control", "action": "inspect_changes", "changeset_id": "cs-1"})),
        ),
        case(
            "changes",
            "/changes cs-1 inspect",
            Engine(json!({"type": "agent_control", "action": "inspect_changes", "changeset_id": "cs-1"})),
        ),
        case(
            "changes",
            "/changes cs-1 apply",
            Engine(json!({"type": "agent_control", "action": "apply_changes", "changeset_id": "cs-1"})),
        ),
        case(
            "changes",
            "/changes cs-1 discard",
            Engine(json!({"type": "agent_control", "action": "discard_changes", "changeset_id": "cs-1"})),
        ),
        case(
            "changes",
            "/changes cs-1 merge",
            Invalid("action inconnue `merge` : inspect, apply, discard"),
        ),
        case(
            "changes",
            "/changes",
            Invalid("donnez l'identifiant d'un ChangeSet (voir `changes ready`)"),
        ),
        // /jobs: engine jobs of the session, graceful stop by id
        case("jobs", "/jobs", Engine(json!({"type": "agent_control", "action": "jobs"}))),
        case(
            "jobs",
            "/jobs stop job-7",
            Engine(json!({"type": "agent_control", "action": "stop_job", "job_id": "job-7"})),
        ),
        case("jobs", "/jobs stop", Invalid("`/jobs` ou `/jobs stop <identifiant de tâche>`")),
        case("jobs", "/jobs kill job-7", Invalid("`/jobs` ou `/jobs stop <identifiant de tâche>`")),
        // Inspectors (presentations only, no engine call)
        case("memory", "/memory", Pres(json!({"view": "memory"}))),
        case("context", "/context", Pres(json!({"view": "context"}))),
        case("ctx", "/ctx", Pres(json!({"view": "context"}))),
        case("context", "/context details", Pres(json!({"view": "context_details"}))),
        case("ctx", "/ctx details", Pres(json!({"view": "context_details"}))),
        case("context", "/context all", Invalid("argument inconnu `all` : /context ou /context details")),
        case("cost", "/cost", Pres(json!({"view": "cost"}))),
        case("usage", "/usage", Pres(json!({"view": "cost"}))),
        case("cost", "/cost details", Pres(json!({"view": "cost_details"}))),
        case("usage", "/usage details", Pres(json!({"view": "cost_details"}))),
        case("cost", "/cost all", Invalid("argument inconnu `all` : /cost ou /cost details")),
        case("session", "/session", Pres(json!({"view": "session"}))),
        case("diff", "/diff", Pres(json!({"view": "diff"}))),
        case("tools", "/tools", Pres(json!({"view": "tools"}))),
        case("mcp", "/mcp", Pres(json!({"view": "mcp"}))),
        case("config", "/config", Pres(json!({"view": "config"}))),
        // /resume (/sessions): picker without an id; with an id, the desktop open path
        case("resume", "/resume", Pres(json!({"view": "session_picker"}))),
        case("sessions", "/sessions", Pres(json!({"view": "session_picker"}))),
        case(
            "resume",
            "/resume 2026-x",
            Pres(json!({"view": "resume", "session_id": "2026-x"})),
        ),
        case(
            "sessions",
            "/sessions 2026-x",
            Pres(json!({"view": "resume", "session_id": "2026-x"})),
        ),
        // Engine actions
        case("compact", "/compact", Engine(json!({"type": "compact"}))),
        case("clear", "/clear", Engine(json!({"type": "clear_context"}))),
        // Attachments: the path as typed, quotes removed, spaces and Unicode kept
        case(
            "file",
            "/file Mes documents/rapport final é.md",
            Pres(json!({"view": "attach", "kind": "file", "path": "Mes documents/rapport final é.md"})),
        ),
        case(
            "file",
            "/file \"Mon dossier/é.txt\"",
            Pres(json!({"view": "attach", "kind": "file", "path": "Mon dossier/é.txt"})),
        ),
        // Decomposed accents (NFD) are kept as typed: no normalisation in the parser.
        case(
            "file",
            "/file \"Mon dossier/e\u{301}.txt\"",
            Pres(json!({"view": "attach", "kind": "file", "path": "Mon dossier/e\u{301}.txt"})),
        ),
        case("file", "/file \"\"", Invalid("donnez un chemin")),
        case("file", "/file", Invalid("donnez un chemin")),
        case(
            "folder",
            "/folder Projets/2026",
            Pres(json!({"view": "attach", "kind": "folder", "path": "Projets/2026"})),
        ),
        case(
            "folder",
            "/folder \"Données clients/2026 final\"",
            Pres(json!({"view": "attach", "kind": "folder", "path": "Données clients/2026 final"})),
        ),
        case("folder", "/folder", Invalid("donnez un chemin")),
        case(
            "image",
            "/image \"capture écran.png\"",
            Pres(json!({"view": "attach", "kind": "image", "path": "capture écran.png"})),
        ),
        case(
            "img",
            "/img photos/été.png",
            Pres(json!({"view": "attach", "kind": "image", "path": "photos/été.png"})),
        ),
        // Help and quit
        case("help", "/help", Pres(json!({"view": "help"}))),
        case("?", "/?", Pres(json!({"view": "help"}))),
        case("quit", "/quit", Pres(json!({"view": "quit"}))),
        case("exit", "/exit", Pres(json!({"view": "quit"}))),
        case("q", "/q", Pres(json!({"view": "quit"}))),
    ]
}

/// The outcome of one line as the frontend sees it: the wire value, or the error code and message.
type Outcome = Result<Value, (IpcErrorCode, String)>;

async fn outcome(line: &str) -> Outcome {
    match slash_parse(line.to_string()).await {
        Ok(action) => Ok(serde_json::to_value(&action).expect("SlashAction serialises")),
        Err(IpcError { code, message }) => Err((code, message)),
    }
}

async fn parse_ok(line: &str) -> Value {
    match outcome(line).await {
        Ok(v) => v,
        Err((code, message)) => panic!("{line}: expected an action, got {code:?}: {message}"),
    }
}

async fn parse_err(line: &str) -> (IpcErrorCode, String) {
    match outcome(line).await {
        Ok(v) => panic!("{line}: expected an error, got {v}"),
        Err(e) => e,
    }
}

/// The first word of a line, without the leading slash.
fn first_token(line: &str) -> &str {
    line.trim_start_matches('/').split_whitespace().next().unwrap_or("")
}

#[tokio::test]
async fn every_registry_name_and_alias_has_a_matrix_row() {
    let cases = matrix();
    let covered: BTreeSet<&str> = cases.iter().map(|c| c.token).collect();
    let registry: BTreeSet<&str> = COMMANDS
        .iter()
        .flat_map(|c| std::iter::once(c.name).chain(c.aliases.iter().copied()))
        .collect();
    assert_eq!(
        covered, registry,
        "every name and alias of the registry must have a row (and only those)"
    );
    for c in &cases {
        assert_eq!(first_token(c.line), c.token, "row token must start its line: {}", c.line);
    }
}

#[tokio::test]
async fn every_matrix_line_maps_to_its_expected_action() {
    for case in matrix() {
        match &case.want {
            Want::Engine(expected) => {
                let got = parse_ok(case.line).await;
                assert_eq!(&got, &engine(expected.clone()), "{}", case.line);
                // The engine command the frontend forwards must be a real engine `Command`.
                let command = got["command"].clone();
                serde_json::from_value::<Command>(command.clone())
                    .unwrap_or_else(|e| panic!("{}: not an engine Command ({e}): {command}", case.line));
            }
            Want::Present(expected) => {
                assert_eq!(
                    &parse_ok(case.line).await,
                    &present(expected.clone()),
                    "{}",
                    case.line
                );
            }
            Want::Invalid(message) => {
                let (code, got) = parse_err(case.line).await;
                assert_eq!(code, IpcErrorCode::Invalid, "{}", case.line);
                assert_eq!(got.as_str(), *message, "{}", case.line);
            }
        }
    }
}

#[tokio::test]
async fn aliases_act_exactly_like_their_command() {
    // Same argument after the name or after an alias: same action, or the same error.
    let arguments = [
        "",
        "details",
        "deep",
        "default",
        "re:x",
        "pdf hello",
        "stop job-1",
        "reload",
        "status a1",
        "all",
        "apply",
        "\"Mon dossier/é.txt\"",
    ];
    for c in COMMANDS {
        for alias in c.aliases {
            for argument in arguments {
                let sep = if argument.is_empty() { "" } else { " " };
                let by_name = outcome(&format!("/{}{sep}{argument}", c.name)).await;
                let by_alias = outcome(&format!("/{alias}{sep}{argument}")).await;
                assert_eq!(by_alias, by_name, "/{alias}{sep}{argument} differs from /{}", c.name);
            }
        }
    }
}

#[tokio::test]
async fn resume_with_an_id_never_becomes_an_engine_resume() {
    for id in ["2026-x", "sess_01", "séance-été-1"] {
        for word in ["resume", "sessions"] {
            let got = parse_ok(&format!("/{word} {id}")).await;
            assert_eq!(
                got,
                present(json!({"view": "resume", "session_id": id })),
                "/{word} {id}"
            );
            assert_ne!(got["kind"], json!("engine"), "/{word} {id} must not be an engine command");
        }
    }
}

#[tokio::test]
async fn skill_without_a_request_is_an_error_and_sends_nothing() {
    for line in ["/skill x", "/skill x   ", "/skill x \t "] {
        let (code, message) = parse_err(line).await;
        assert_eq!(code, IpcErrorCode::Invalid, "{line}");
        assert!(message.contains("donnez la demande"), "{line}: {message}");
    }
    let (code, _) = parse_err("/skill").await;
    assert_eq!(code, IpcErrorCode::Invalid);
}

#[tokio::test]
async fn effort_default_resets_the_profile_to_null() {
    let got = parse_ok("/effort default").await;
    assert_eq!(got, engine(json!({"type": "set_reasoning", "profile": null})));
}

#[tokio::test]
async fn search_regex_flag_follows_the_re_prefix() {
    assert_eq!(
        parse_ok("/search re:a.b").await,
        engine(json!({"type": "search", "text": "a.b", "regex": true}))
    );
    assert_eq!(
        parse_ok("/search a.b").await,
        engine(json!({"type": "search", "text": "a.b", "regex": false}))
    );
}

#[tokio::test]
async fn changes_and_jobs_carry_their_ids_to_the_engine() {
    for (line, action) in [
        ("/changes cs-9 inspect", "inspect_changes"),
        ("/changes cs-9 apply", "apply_changes"),
        ("/changes cs-9 discard", "discard_changes"),
    ] {
        assert_eq!(
            parse_ok(line).await,
            engine(json!({"type": "agent_control", "action": action, "changeset_id": "cs-9"})),
            "{line}"
        );
    }
    assert_eq!(
        parse_ok("/jobs stop job-7").await,
        engine(json!({"type": "agent_control", "action": "stop_job", "job_id": "job-7"}))
    );
}

#[tokio::test]
async fn attachment_paths_keep_spaces_and_unicode_as_typed() {
    let got = parse_ok("/file \"Mon dossier/é.txt\"").await;
    assert_eq!(got["present"]["path"], json!("Mon dossier/é.txt"));
    assert_eq!(got["present"]["kind"], json!("file"));

    // The same bytes come back: no trimming inside the path, no normalisation.
    let decomposed = "Mon dossier/e\u{301}.txt";
    let got = parse_ok(&format!("/file \"{decomposed}\"")).await;
    assert_eq!(got["present"]["path"], json!(decomposed));
    assert_ne!(decomposed, "Mon dossier/é.txt");
}

#[tokio::test]
async fn unknown_commands_and_bad_lines_are_invalid_in_french() {
    let (code, message) = parse_err("/nope").await;
    assert_eq!(code, IpcErrorCode::Invalid);
    assert_eq!(message, "commande inconnue /nope (essayez /help)");

    // Commands are case-sensitive, as in the terminal.
    let (code, message) = parse_err("/Model").await;
    assert_eq!(code, IpcErrorCode::Invalid);
    assert_eq!(message, "commande inconnue /Model (essayez /help)");

    let (code, message) = parse_err("model").await;
    assert_eq!(code, IpcErrorCode::Invalid);
    assert_eq!(message, "ce n'est pas une commande");

    let (code, _) = parse_err("/").await;
    assert_eq!(code, IpcErrorCode::Invalid);
}

#[tokio::test]
async fn surrounding_spaces_do_not_change_the_action() {
    assert_eq!(parse_ok("   /model   ").await, parse_ok("/model").await);
    assert_eq!(parse_ok("\t/quit\n").await, parse_ok("/quit").await);
}

#[tokio::test]
async fn the_palette_lists_every_registry_entry_in_order() {
    let list = slash_commands().await.expect("slash_commands");
    assert_eq!(list.len(), COMMANDS.len());
    for (info, c) in list.iter().zip(COMMANDS.iter()) {
        assert_eq!(info.name, c.name);
        assert_eq!(info.aliases, c.aliases.iter().map(|a| a.to_string()).collect::<Vec<_>>());
        assert_eq!(info.args, c.args);
        assert_eq!(info.description, c.description_fr);
        assert!(!info.description.is_empty(), "/{}", c.name);
    }
}
