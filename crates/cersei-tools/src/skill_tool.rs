//! The `Skill` tool: list the skills the agent may use, activate one (read
//! its full instructions), or locate one of its files. Everything goes
//! through the agent's [`SkillSession`] (the `bricks-skills` service and
//! this agent's ledger); without one, the tool says so.
//!
//! Activating reads instructions; it runs nothing, changes no working
//! directory and grants no permission. The tool only *proposes* the
//! activation: the engine records it once the text is in the agent's
//! context (and refuses it, without cutting it, when it does not fit).

use super::*;
use bricks_skills::{
    render_activation, ActivationError, Ledger, Origin, ResolveError, SkillAccess,
};
use serde::Deserialize;
use std::sync::Arc;

/// The skills of one agent: the project's service and trust (shared with
/// its sub-agents) and its own ledger (never shared).
#[derive(Clone)]
pub struct SkillSession {
    pub access: SkillAccess,
    pub ledger: Arc<parking_lot::Mutex<Ledger>>,
}

impl SkillSession {
    pub fn new(access: SkillAccess) -> Self {
        Self {
            access,
            ledger: Arc::new(parking_lot::Mutex::new(Ledger::default())),
        }
    }
}

/// Metadata key of a proposed activation in a tool result.
pub const ACTIVATION_KEY: &str = "skill_activation";

pub struct SkillTool;

impl SkillTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SkillTool {
    fn default() -> Self {
        Self::new()
    }
}

const PER_PAGE: usize = 20;

fn resolve_error(e: &ActivationError, access: &SkillAccess) -> ToolResult {
    match e {
        ActivationError::Resolve(ResolveError::NotFound { name }) => {
            let (all, _) = access.search("", 0, usize::MAX);
            let names: Vec<String> = all.into_iter().map(|c| c.name).collect();
            crate::tool_feedback::not_found(
                "skill",
                name,
                &names,
                "Use action 'list' to see the skills you can activate.",
            )
        }
        other => ToolResult::error(other.to_string()),
    }
}

#[async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &str {
        "Skill"
    }

    fn description(&self) -> &str {
        "Activate a skill to read its full instructions (action 'activate'), list the skills \
         you can activate (action 'list'), or locate one of a skill's files (action \
         'resource'). Activation runs nothing and grants no permission."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Custom
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["activate", "list", "resource"],
                    "description": "'activate' (default when `skill` is given), 'list', or 'resource'"
                },
                "skill": {
                    "type": "string",
                    "description": "The skill's name (exactly as listed)"
                },
                "args": {
                    "type": "string",
                    "description": "Optional: the request to apply the skill to. Given back separately; nothing in the instructions is replaced."
                },
                "query": {
                    "type": "string",
                    "description": "For 'list': words to filter names and descriptions"
                },
                "page": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "For 'list': page number (20 per page)"
                },
                "path": {
                    "type": "string",
                    "description": "For 'resource': a path relative to the skill's directory"
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            action: Option<String>,
            skill: Option<String>,
            args: Option<String>,
            query: Option<String>,
            page: Option<usize>,
            path: Option<String>,
        }
        let input: Input = match crate::tool_feedback::parse_input(self, &input) {
            Ok(i) => i,
            Err(e) => return e,
        };
        let Some(session) = ctx.extensions.get::<SkillSession>() else {
            return ToolResult::error(
                "Skills are not configured for this agent: no skill can be activated.",
            );
        };
        let skill = input
            .skill
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        // Without an action, `skill: "list"` keeps its historical meaning
        // (listing). With `action: "activate"`, a skill named `list` is
        // activated like any other.
        let action = match (input.action.as_deref().map(str::trim), skill) {
            (Some(a), _) => a.to_string(),
            (None, Some("list")) | (None, None) => "list".into(),
            (None, Some(_)) => "activate".into(),
        };
        match action.as_str() {
            "list" => {
                let page = input.page.unwrap_or(0);
                let (entries, total) =
                    session
                        .access
                        .search(input.query.as_deref().unwrap_or(""), page, PER_PAGE);
                if total == 0 {
                    return ToolResult::success("No skill can be activated here.");
                }
                let mut out = format!(
                    "{total} skill(s) can be activated (page {page}, {PER_PAGE} per page):\n"
                );
                for e in entries {
                    out.push_str(&format!("- {}: {}\n", e.name, e.description));
                }
                ToolResult::success(out)
            }
            "activate" => {
                let Some(name) = skill else {
                    return ToolResult::error("`skill` is required to activate a skill.");
                };
                let activation = match session.access.activate(name) {
                    Ok(a) => a,
                    Err(e) => return resolve_error(&e, &session.access),
                };
                let request = input
                    .args
                    .as_deref()
                    .map(str::trim)
                    .filter(|a| !a.is_empty())
                    .map(|a| format!("\n\nRequest to apply it to (from the call): {a}"))
                    .unwrap_or_default();
                // Already active with this exact text: not repeated.
                if let Some(active) = session
                    .ledger
                    .lock()
                    .active_revision(&activation.source, &activation.revision)
                {
                    return ToolResult::success(format!(
                        "Skill `{}` is already active (revision {}, activation {}); its \
                         instructions above still apply.{request}",
                        activation.name, activation.revision, active.id
                    ));
                }
                let meta = serde_json::json!({
                    "name": activation.name,
                    "source": activation.source,
                    "revision": activation.revision,
                });
                let text = {
                    let mut ledger = session.ledger.lock();
                    let id = ledger.propose(activation.clone(), Origin::Model);
                    let text = render_activation(&activation, &id, Some(&ctx.working_dir));
                    (id, text)
                };
                let (id, body) = text;
                let mut meta = meta;
                meta[ACTIVATION_KEY] = serde_json::json!(id);
                ToolResult::success(format!("{body}{request}")).with_metadata(meta)
            }
            "resource" => {
                let (Some(name), Some(path)) = (skill, input.path.as_deref()) else {
                    return ToolResult::error("'resource' needs `skill` and `path`.");
                };
                match session.access.resource(name, path) {
                    Ok(p) => ToolResult::success(format!(
                        "{} — read it with `Read` (its permissions apply). Nothing was read or run.",
                        p.display()
                    )),
                    Err(e) => ToolResult::error(e),
                }
            }
            other => ToolResult::error(format!(
                "unknown action `{other}`: 'activate', 'list' or 'resource'"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::AllowAll;
    use bricks_skills::{standard_roots, ProjectDecision, SkillsService};
    use std::path::Path;

    fn write(root: &Path, dir: &str, front: &str, body: &str) {
        let d = root.join(dir);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("SKILL.md"), format!("---\n{front}---\n{body}")).unwrap();
    }

    fn ctx(wd: &Path, session: Option<SkillSession>) -> ToolContext {
        let extensions = Extensions::default();
        if let Some(s) = session {
            extensions.insert(s);
        }
        ToolContext {
            working_dir: wd.to_path_buf(),
            session_id: "skill-test".into(),
            permissions: Arc::new(AllowAll),
            cost_tracker: Arc::new(CostTracker::new()),
            mcp_manager: None,
            extensions,
        }
    }

    fn session(project: &Path, home: &Path) -> SkillSession {
        let svc = SkillsService::new(
            standard_roots(project, Some(home)),
            &[],
            Some(project.into()),
        );
        SkillSession::new(SkillAccess::new(Arc::new(svc)))
    }

    #[tokio::test]
    async fn without_a_service_the_tool_says_so() {
        let t = tempfile::tempdir().unwrap();
        let r = SkillTool
            .execute(serde_json::json!({"skill": "x"}), &ctx(t.path(), None))
            .await;
        assert!(r.is_error && r.content.contains("not configured"));
    }

    #[tokio::test]
    async fn activation_is_proposed_not_committed_and_args_stay_separate() {
        let t = tempfile::tempdir().unwrap();
        let (p, h) = (t.path().join("p"), t.path().join("h"));
        write(
            &h.join(".agents/skills"),
            "deploy",
            "name: deploy\ndescription: Deploy\n",
            "Deploy $ARGUMENTS carefully.\n",
        );
        let s = session(&p, &h);
        let c = ctx(&p, Some(s.clone()));
        let r = SkillTool
            .execute(
                serde_json::json!({"action": "activate", "skill": "deploy", "args": "staging"}),
                &c,
            )
            .await;
        assert!(!r.is_error, "{}", r.content);
        assert!(
            r.content.contains("Deploy $ARGUMENTS carefully."),
            "not substituted"
        );
        assert!(r
            .content
            .contains("Request to apply it to (from the call): staging"));
        let id = r.metadata.as_ref().unwrap()[ACTIVATION_KEY]
            .as_str()
            .unwrap()
            .to_string();
        assert!(r.content.contains(&id));
        assert_eq!(s.ledger.lock().active().count(), 0, "only proposed");
        assert!(s.ledger.lock().take_pending(&id).is_some());
    }

    #[tokio::test]
    async fn list_actions_and_a_skill_named_list() {
        let t = tempfile::tempdir().unwrap();
        let (p, h) = (t.path().join("p"), t.path().join("h"));
        write(
            &h.join(".agents/skills"),
            "list",
            "name: list\ndescription: A skill called list\n",
            "LIST-BODY",
        );
        let s = session(&p, &h);
        let c = ctx(&p, Some(s));
        let r = SkillTool
            .execute(serde_json::json!({"skill": "list"}), &c)
            .await;
        assert!(r.content.contains("can be activated") && !r.content.contains("LIST-BODY"));
        let r = SkillTool
            .execute(
                serde_json::json!({"action": "activate", "skill": "list"}),
                &c,
            )
            .await;
        assert!(r.content.contains("LIST-BODY"));
    }

    #[tokio::test]
    async fn errors_are_distinct() {
        let t = tempfile::tempdir().unwrap();
        let (p, h) = (t.path().join("p"), t.path().join("h"));
        write(
            &p.join(".agents/skills"),
            "local",
            "name: local\ndescription: d\n",
            "b",
        );
        write(&h.join(".agents/skills"), "bad", "name: bad\n", "b");
        write(
            &h.join(".agents/skills"),
            "fork",
            "name: fork\ndescription: d\ncontext: fork\n",
            "b",
        );
        let s = session(&p, &h);
        let c = ctx(&p, Some(s.clone()));
        let run = |v: Value| {
            let c = &c;
            async move { SkillTool.execute(v, c).await }
        };
        let r = run(serde_json::json!({"skill": "nope"})).await;
        assert!(
            r.is_error && r.content.contains("not found"),
            "{}",
            r.content
        );
        let r = run(serde_json::json!({"skill": "local"})).await;
        assert!(
            r.is_error && r.content.contains("not approved"),
            "{}",
            r.content
        );
        let r = run(serde_json::json!({"skill": "bad"})).await;
        assert!(r.is_error && r.content.contains("invalid"), "{}", r.content);
        let r = run(serde_json::json!({"skill": "fork"})).await;
        assert!(
            r.is_error && r.content.contains("incompatible"),
            "{}",
            r.content
        );
        // The model cannot approve the project: there is no such action.
        let r = run(serde_json::json!({"action": "trust", "skill": "local"})).await;
        assert!(r.is_error);
        assert_eq!(s.access.trust().project, ProjectDecision::Pending);
    }

    #[tokio::test]
    async fn resources_are_located_not_read() {
        let t = tempfile::tempdir().unwrap();
        let (p, h) = (t.path().join("p"), t.path().join("h"));
        let root = h.join(".agents/skills");
        write(&root, "pdf", "name: pdf\ndescription: d\n", "b");
        std::fs::create_dir_all(root.join("pdf/references")).unwrap();
        std::fs::write(root.join("pdf/references/guide.md"), "GUIDE").unwrap();
        let c = ctx(&p, Some(session(&p, &h)));
        let r = SkillTool
            .execute(
                serde_json::json!({"action": "resource", "skill": "pdf", "path": "references/guide.md"}),
                &c,
            )
            .await;
        assert!(!r.is_error && r.content.contains("guide.md") && !r.content.contains("GUIDE"));
        let r = SkillTool
            .execute(
                serde_json::json!({"action": "resource", "skill": "pdf", "path": "../../x"}),
                &c,
            )
            .await;
        assert!(r.is_error);
    }
}
