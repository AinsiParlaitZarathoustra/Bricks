//! `[bash] extra_path` through the controller and real processes: what is
//! announced to the model is the `PATH` Bash really has, and a sub-agent's
//! Bash has the directories too. Fake executables only.

#![cfg(unix)]

#[allow(dead_code)]
mod common;

use bricks_agent::BricksConfig;
use bricks_agent::agents::ProfileSources;
use bricks_agent::control::scripted::{Reply, Script, ScriptedCatalog};
use bricks_agent::control::*;
use bricks_agent::permission::{PermissionProfile, SessionPolicy};
use bricks_agent::system_prompt::SystemPromptOptions;
use bricks_provider::{CompletionRequest, Provider};
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

const LISTED: &str = "Exécutables détectés (hors outils Bricks) : ";

/// The scripted models; each waits for the sub-agents it was given.
struct Follow(
    Arc<ScriptedCatalog>,
    Arc<common::follow::Waited>,
    Arc<AtomicBool>,
);

impl ModelCatalog for Follow {
    fn models(&self) -> Vec<ModelChoice> {
        self.0.models()
    }
    fn build(&self, selection: &str, reasoning: Option<&str>) -> Result<Box<dyn Provider>, String> {
        Ok(Box::new(common::follow::FollowHandles {
            inner: self.0.build(selection, reasoning)?,
            selection: selection.to_string(),
            waited: Arc::clone(&self.1),
            enabled: Arc::clone(&self.2),
        }))
    }
}

/// A project whose prompt is assembled for each request, with `[bash]
/// extra_path`.
struct Loader {
    extra_path: Vec<String>,
}

impl ProjectLoader for Loader {
    fn load(&self, wd: &Path) -> Result<ProjectContext, String> {
        let mut bricks = BricksConfig::default();
        bricks.agent.model = Some("test/a".into());
        bricks.semantic.lsp.enabled = false;
        bricks.bash.extra_path = self.extra_path.clone();
        Ok(ProjectContext {
            working_dir: wd.to_path_buf(),
            prompt_options: Some(SystemPromptOptions {
                is_non_interactive: true,
                ..Default::default()
            }),
            bricks,
            system_prompt: None,
            long_term_memory: None,
            memory_space: None,
            mcp_servers: Vec::new(),
            agent_profile_sources: Some(ProfileSources {
                project_dir: Some(wd.join(".bricks/agents")),
                user_dir: None,
            }),
        })
    }
}

async fn run(extra_path: &Path, replies: Vec<Reply>) -> Vec<CompletionRequest> {
    let dir = tempfile::tempdir().unwrap();
    let wd = std::fs::canonicalize(dir.path()).unwrap();
    let profiles = wd.join(".bricks/agents");
    std::fs::create_dir_all(&profiles).unwrap();
    std::fs::write(
        profiles.join("marker.md"),
        "---\nname: marker\ndescription: A marker profile.\n---\nInstructions.\n",
    )
    .unwrap();
    let script = Script::new(replies);
    let mut bricks = BricksConfig::default();
    bricks.agent.model = Some("test/a".into());
    let mut cfg = EngineConfig::new(
        &wd,
        Arc::new(Follow(
            ScriptedCatalog::new(&["a"], script.clone()),
            Arc::new(common::follow::Waited::default()),
            Arc::new(AtomicBool::new(true)),
        )),
        bricks,
        wd.join(".sessions"),
    );
    cfg.interactive = false;
    cfg.permissions = Some(common::default_store(
        &wd,
        &SessionPolicy::chosen(PermissionProfile::Execute),
    ));
    cfg.project_loader = Some(Arc::new(Loader {
        extra_path: vec![extra_path.display().to_string()],
    }));
    let (ctl, mut events) = Controller::open(
        cfg,
        OpenOptions {
            session: SessionChoice::New,
            model: None,
            reasoning: None,
        },
    )
    .await
    .unwrap();
    ctl.send(Command::Submit {
        prompt: Prompt::text("go"),
    })
    .unwrap();
    loop {
        let e = tokio::time::timeout(Duration::from_secs(30), events.next())
            .await
            .expect("an event")
            .expect("open");
        if matches!(e.event, Event::RunFinished { .. }) {
            break;
        }
    }
    let requests = script.requests().into_iter().map(|(_, r)| r).collect();
    ctl.close().await;
    requests
}

fn fake(dir: &Path, name: &str, says: &str) {
    let file = dir.join(name);
    std::fs::write(&file, format!("#!/bin/sh\necho {says}\n")).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn listed(request: &CompletionRequest) -> Vec<String> {
    let system = request.system.as_deref().unwrap_or("");
    system
        .lines()
        .find_map(|l| l.strip_prefix(LISTED))
        .map(|l| l.split(", ").map(str::to_string).collect())
        .unwrap_or_default()
}

/// The list comes from the `PATH` of the session: with the extra directory
/// before the first command, and whatever a command makes of it afterwards.
#[tokio::test]
async fn the_programs_announced_follow_the_path_of_the_session() {
    // A program of the announced list this machine does not have.
    let Some(missing) = [
        "java", "docker", "go", "make", "python3", "npm", "node", "rg", "cargo", "git",
    ]
    .into_iter()
    .find(|name| which::which(name).is_err()) else {
        return;
    };
    let extra = tempfile::tempdir().unwrap();
    fake(extra.path(), missing, "fake");
    let only_cargo = tempfile::tempdir().unwrap();
    fake(only_cargo.path(), "cargo", "fake");
    let command = format!("export PATH={}", only_cargo.path().display());
    let requests = run(
        extra.path(),
        vec![
            Reply::tool("c1", "Bash", json!({ "command": command })),
            Reply::text("done"),
        ],
    )
    .await;
    assert!(
        listed(&requests[0]).iter().any(|e| e == missing),
        "{:?}",
        listed(&requests[0])
    );
    assert_eq!(listed(&requests[1]), ["cargo"]);
}

/// A relative entry of `PATH`, set in Bash, depends on where its `cd` left
/// the session.
#[tokio::test]
async fn a_relative_entry_is_detected_from_the_directory_of_the_session() {
    let extra = tempfile::tempdir().unwrap();
    let setup = "mkdir -p a/rel && printf '#!/bin/sh\\n' > a/rel/cargo \
                 && chmod +x a/rel/cargo && export PATH=rel";
    let requests = run(
        extra.path(),
        vec![
            Reply::tool("c1", "Bash", json!({ "command": setup })),
            Reply::tool("c2", "Bash", json!({ "command": "cd a" })),
            Reply::tool("c3", "Bash", json!({ "command": "cd .." })),
            Reply::text("done"),
        ],
    )
    .await;
    assert!(
        listed(&requests[1]).is_empty(),
        "{:?}",
        listed(&requests[1])
    );
    assert_eq!(listed(&requests[2]), ["cargo"]);
    assert!(
        listed(&requests[3]).is_empty(),
        "{:?}",
        listed(&requests[3])
    );
}

/// A command that installs or removes a program in a directory of `PATH`
/// changes what the next request announces.
#[tokio::test]
async fn a_program_installed_during_the_session_is_announced() {
    let extra = tempfile::tempdir().unwrap();
    let bin = tempfile::tempdir().unwrap();
    let bin = bin.path().display();
    let install = format!("printf '#!/bin/sh\\n' > {bin}/cargo && /bin/chmod +x {bin}/cargo");
    let requests = run(
        extra.path(),
        vec![
            Reply::tool(
                "c1",
                "Bash",
                json!({ "command": format!("export PATH={bin}") }),
            ),
            Reply::tool("c2", "Bash", json!({ "command": install })),
            Reply::tool(
                "c3",
                "Bash",
                json!({ "command": format!("/bin/rm {bin}/cargo") }),
            ),
            Reply::text("done"),
        ],
    )
    .await;
    assert!(
        listed(&requests[1]).is_empty(),
        "{:?}",
        listed(&requests[1])
    );
    assert_eq!(listed(&requests[2]), ["cargo"]);
    assert!(
        listed(&requests[3]).is_empty(),
        "{:?}",
        listed(&requests[3])
    );
}

#[tokio::test]
async fn a_sub_agent_has_the_extra_directory() {
    let extra = tempfile::tempdir().unwrap();
    fake(extra.path(), "bricks-fake-tool", "MARK-FAKE-TOOL");
    let requests = run(
        extra.path(),
        vec![
            Reply::tool(
                "c1",
                "Agent",
                json!({"task": "Look.", "profile": "marker", "isolation": "shared"}),
            ),
            Reply::tool("c2", "Bash", json!({ "command": "bricks-fake-tool" })),
            Reply::text("child result"),
            Reply::text("done"),
        ],
    )
    .await;
    assert!(
        requests
            .iter()
            .any(|r| format!("{:?}", r.messages).contains("MARK-FAKE-TOOL")),
        "the child's Bash did not find the tool"
    );
}
