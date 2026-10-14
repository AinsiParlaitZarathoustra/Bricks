//! The system prompt as it is really sent (Sprint 13.2): through the
//! controller and a scripted provider, the session agent's request and a
//! sub-agent's request carry the Bricks foundation once, with the bullets
//! their own tools decide; the child keeps its role and profile; custom
//! sections, the skills catalog and recalled memory survive once. These
//! are contract tests of the text, not of any model's behaviour.

use async_trait::async_trait;
use cersei_agent::BricksConfig;
use cersei_agent::agents::ProfileSources;
use cersei_agent::control::scripted::{Reply, Script, ScriptedCatalog};
use cersei_agent::control::*;
use cersei_agent::system_prompt::{
    SYSTEM_PROMPT_DYNAMIC_BOUNDARY, SystemPromptOptions, build_system_prompt,
};
use cersei_memory::{LongTermMemory, MaintenanceReport, MemoryTurn, RecalledContext};
use cersei_provider::CompletionRequest;
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const REFERENCE: &str = include_str!("fixtures/bricks_foundation.txt");

fn reference(code_scout: bool, delegation: bool) -> String {
    REFERENCE
        .trim_end_matches('\n')
        .lines()
        .filter(|l| code_scout || !l.starts_with("- Utilise CodeScout lorsque"))
        .filter(|l| delegation || !l.starts_with("- Délègue aux sous-agents"))
        .collect::<Vec<_>>()
        .join("\n")
}

const INTRO: &str = "Tu es Bricks, un agent de développement logiciel autonome.";

/// Recalls a fixed marker.
struct Recall;

#[async_trait]
impl LongTermMemory for Recall {
    async fn recall_context(
        &self,
        _query: &str,
        _max_tokens: usize,
    ) -> cersei_types::Result<Option<RecalledContext>> {
        Ok(Some(RecalledContext {
            text: "<recalled>MARK-RECALL</recalled>".into(),
            tokens: 5,
            items: 1,
            omitted: 0,
        }))
    }
    async fn record_turns(&self, _: Option<&str>, _: &[MemoryTurn]) -> cersei_types::Result<()> {
        Ok(())
    }
    async fn maintain(
        &self,
        _: &tokio_util::sync::CancellationToken,
    ) -> cersei_types::Result<MaintenanceReport> {
        Ok(MaintenanceReport::default())
    }
}

/// A project as a frontend loads it: the prompt built for the tools the
/// session agent announces (as the CLI does), custom sections, a memory.
struct Loader {
    agents: bool,
}

impl ProjectLoader for Loader {
    fn load(&self, wd: &Path) -> Result<ProjectContext, String> {
        let mut bricks = BricksConfig::default();
        bricks.agent.model = Some("test/a".into());
        bricks.semantic.lsp.enabled = false;
        bricks.agents.enabled = self.agents;
        bricks
            .permissions
            .tools
            .insert("Agent".into(), Action::Allow);
        let mut tools: Vec<String> = cersei_tools::coding()
            .iter()
            .map(|t| t.name().to_string())
            .collect();
        if self.agents {
            tools.extend(["Agent", "Agents", "AgentControl", "AgentProfiles"].map(String::from));
        }
        let system = build_system_prompt(&SystemPromptOptions {
            is_non_interactive: true,
            working_directory: Some(wd.display().to_string()),
            tools_available: tools,
            custom_system_prompt: Some("MARK-CUSTOM".into()),
            extra_cached_sections: vec![("rules".into(), "MARK-CACHED".into())],
            extra_dynamic_sections: vec![("ctx".into(), "MARK-DYNAMIC".into())],
            has_auto_compact: true,
            ..Default::default()
        });
        Ok(ProjectContext {
            working_dir: wd.to_path_buf(),
            bricks,
            system_prompt: Some(system),
            long_term_memory: Some(Arc::new(Recall)),
            memory_space: None,
            mcp_servers: Vec::new(),
            agent_profile_sources: Some(ProfileSources {
                project_dir: Some(wd.join(".bricks/agents")),
                user_dir: None,
            }),
        })
    }
}

async fn run(agents: bool, replies: Vec<Reply>) -> Vec<CompletionRequest> {
    let dir = tempfile::tempdir().unwrap();
    let wd = std::fs::canonicalize(dir.path()).unwrap();
    let home = tempfile::tempdir().unwrap();
    let skill = home.path().join(".agents/skills/marker-skill");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: marker-skill\ndescription: MARK-SKILL procedure.\n---\nBody.\n",
    )
    .unwrap();
    let profiles = wd.join(".bricks/agents");
    std::fs::create_dir_all(&profiles).unwrap();
    std::fs::write(
        profiles.join("marker.md"),
        "---\nname: marker\ndescription: A marker profile.\n---\nMARK-PROFILE instructions.\n",
    )
    .unwrap();
    let script = Script::new(replies);
    let mut bricks = BricksConfig::default();
    bricks.agent.model = Some("test/a".into());
    let mut cfg = EngineConfig::new(
        &wd,
        ScriptedCatalog::new(&["a"], script.clone()),
        bricks,
        wd.join(".sessions"),
    );
    cfg.skills = Some(SkillsSetup::with_home(Some(home.path().to_path_buf())));
    cfg.project_loader = Some(Arc::new(Loader { agents }));
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
        let e = tokio::time::timeout(Duration::from_secs(20), events.next())
            .await
            .expect("an event")
            .expect("open");
        if matches!(e.event, Event::RunFinished { .. }) {
            break;
        }
    }
    let reqs = script.requests().into_iter().map(|(_, r)| r).collect();
    ctl.close().await;
    reqs
}

fn system(r: &CompletionRequest) -> &str {
    r.system.as_deref().unwrap_or("")
}

fn is_child(r: &CompletionRequest) -> bool {
    system(r).contains("You are a sub-agent working on one task")
}

#[tokio::test]
async fn the_session_and_its_child_send_the_foundation_with_their_own_bullets() {
    let reqs = run(
        true,
        vec![
            Reply::tool("c1", "Agent", json!({"task": "Look.", "profile": "marker"})),
            Reply::text("child result"),
            Reply::text("done"),
        ],
    )
    .await;
    let parent = reqs
        .iter()
        .find(|r| !is_child(r))
        .expect("a parent request");
    let child = reqs.iter().find(|r| is_child(r)).expect("a child request");

    // The session agent: the foundation first, whole, once.
    let p = system(parent);
    assert!(p.starts_with(&reference(true, true)), "{p}");
    assert_eq!(p.matches(INTRO).count(), 1);
    assert_eq!(p.matches(SYSTEM_PROMPT_DYNAMIC_BOUNDARY).count(), 1);
    assert!(!p.contains("## Your role"));
    for mark in [
        "MARK-CUSTOM",
        "MARK-CACHED",
        "MARK-DYNAMIC",
        "MARK-RECALL",
        "MARK-SKILL",
    ] {
        assert_eq!(p.matches(mark).count(), 1, "{mark} in the parent's prompt");
    }
    let boundary = p.find(SYSTEM_PROMPT_DYNAMIC_BOUNDARY).unwrap();
    assert!(p.find("MARK-CUSTOM").unwrap() < boundary);
    assert!(p.find("MARK-DYNAMIC").unwrap() > boundary);
    // The catalog and the recall come after the built prompt.
    assert!(p.find("MARK-SKILL").unwrap() > p.find("MARK-DYNAMIC").unwrap());
    assert!(p.find("MARK-RECALL").unwrap() > p.find("MARK-DYNAMIC").unwrap());
    assert_eq!(p.matches("## Skills").count(), 1);

    // The child: the same foundation, decided by its own tools (it has the
    // delegation tools of the runtime), its role once, its profile once,
    // nothing of the parent's custom sections.
    let c = system(child);
    let names: Vec<&str> = child.tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"Agent") && names.contains(&"CodeScout"));
    assert!(c.starts_with(&reference(true, true)), "{c}");
    assert_eq!(c.matches(INTRO).count(), 1);
    assert_eq!(c.matches("## Your role").count(), 1);
    assert_eq!(c.matches("MARK-PROFILE").count(), 1);
    assert_eq!(c.matches(SYSTEM_PROMPT_DYNAMIC_BOUNDARY).count(), 1);
    assert!(c.find("## Your role").unwrap() < c.find(SYSTEM_PROMPT_DYNAMIC_BOUNDARY).unwrap());
    for mark in ["MARK-CUSTOM", "MARK-CACHED", "MARK-DYNAMIC"] {
        assert!(!c.contains(mark), "{mark} leaked to the child");
    }
}

#[tokio::test]
async fn without_delegation_tools_the_bullet_is_absent_from_the_request() {
    let reqs = run(false, vec![Reply::text("hi")]).await;
    let r = &reqs[0];
    assert!(
        !r.tools
            .iter()
            .any(|t| t.name == "Agent" || t.name == "Agents")
    );
    let s = system(r);
    assert!(s.starts_with(&reference(true, false)), "{s}");
    assert!(!s.contains("Délègue"));
    assert_eq!(s.matches("MARK-RECALL").count(), 1);
}

#[test]
fn a_child_prompt_without_delegation_tools_has_no_delegation_bullet() {
    let general = cersei_agent::agents::ProfileRegistry::built_in()
        .get("inspecteur")
        .expect("a built-in profile");
    assert!(!general.instructions.trim().is_empty());
    let wd = Path::new("/w");
    let without = cersei_agent::agents::spawn::child_system_prompt(
        &general,
        &["Read".to_string(), "Bash".to_string()],
        wd,
    );
    assert!(without.starts_with(&reference(false, false)));
    assert_eq!(without.matches("## Your role").count(), 1);
    assert_eq!(without.matches(general.instructions.trim()).count(), 1);
    let with = cersei_agent::agents::spawn::child_system_prompt(
        &general,
        &["Read".to_string(), "Agents".to_string()],
        wd,
    );
    assert!(with.starts_with(&reference(false, true)));
    assert_eq!(with.matches("## Your role").count(), 1);
}
