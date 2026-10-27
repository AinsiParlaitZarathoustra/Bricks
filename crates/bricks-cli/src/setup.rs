//! Configuration loading for the command line: the workspace and the
//! providers file come from the arguments; everything else is assembled by
//! `bricks-config`, shared with the other frontends. Errors are complete
//! messages for the user (exit code 2).
//!
//! The workspace given on the command line (`--workspace`, alias `--cd`)
//! is where a new session starts. A project's settings (`bricks.toml`,
//! system prompt, long-term memory) are read by `bricks_config::Projects`
//! for the folder of the session actually opened — a resumed session's own
//! folder, not the one Bricks was started in.

use crate::args::Global;
use bricks_agent::control::{EngineConfig, OpenOptions, SessionChoice};
use bricks_provider::ProviderRegistry;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use bricks_config::sessions_dir;

/// The workspace to open: `arg` (or `launch` without one), with a leading
/// `~` or `~/` expanded from `home` (no shell is involved, so a quoted `~`
/// is expanded too; `~user` and variables are not), a relative path taken
/// from `launch`, then made canonical and checked to be a readable folder.
pub fn resolve_workspace(
    launch: &Path,
    arg: Option<&Path>,
    home: Option<&Path>,
) -> Result<PathBuf, String> {
    let given = match arg {
        None => launch.to_path_buf(),
        Some(p) => {
            let mut parts = p.components();
            let expanded = match parts.next() {
                Some(std::path::Component::Normal(first)) if first == "~" => {
                    let home = home.ok_or_else(|| {
                        format!(
                            "cannot expand `~` in {}: the home folder is unknown; give the full \
                             path",
                            p.display()
                        )
                    })?;
                    home.join(parts.as_path())
                }
                Some(std::path::Component::Normal(first))
                    if first.as_encoded_bytes().starts_with(b"~") =>
                {
                    return Err(format!(
                        "`{}`: only `~` and `~/…` are expanded (not `~user`); give the full path",
                        p.display()
                    ));
                }
                _ => p.to_path_buf(),
            };
            if expanded.is_absolute() {
                expanded
            } else {
                launch.join(expanded)
            }
        }
    };
    let canonical = match std::fs::canonicalize(&given) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!(
                "the workspace {} does not exist (nothing is created)",
                given.display()
            ));
        }
        Err(e) => return Err(format!("workspace {}: {e}", given.display())),
    };
    if !canonical.is_dir() {
        return Err(format!(
            "the workspace {} is a file, not a folder",
            canonical.display()
        ));
    }
    std::fs::read_dir(&canonical)
        .map_err(|e| format!("the workspace {} cannot be read: {e}", canonical.display()))?;
    Ok(canonical)
}

/// The workspace of this command line.
pub fn workspace(global: &Global, launch: &Path) -> Result<PathBuf, String> {
    resolve_workspace(
        launch,
        global.workspace.as_deref(),
        dirs::home_dir().as_deref(),
    )
}

/// The providers file: `--providers`, else `$BRICKS_HOME/providers.toml`
/// when `BRICKS_HOME` is set, else the registry's default location.
fn providers_path(global: &Global, launch: &Path) -> Option<PathBuf> {
    global
        .providers
        .as_ref()
        .map(|p| launch.join(p))
        .or_else(bricks_config::providers_file_from_env)
}

pub fn registry(global: &Global, launch: &Path) -> Result<ProviderRegistry, String> {
    bricks_config::load_registry(providers_path(global, launch).as_deref())
}

/// Everything the engine needs. Nothing of a project is read here: the
/// controller asks the project loader for the project of the session it
/// opens (so a broken `bricks.toml` here does not prevent resuming a
/// session of another project).
pub fn engine(global: &Global, launch: &Path, interactive: bool) -> Result<EngineConfig, String> {
    let wd = workspace(global, launch)?;
    let registry = Arc::new(registry(global, launch)?);
    Ok(bricks_config::engine_config(&wd, registry, interactive))
}

pub fn open_options(global: &Global, session: Option<String>) -> OpenOptions {
    OpenOptions {
        session: match session {
            Some(id) => SessionChoice::Resume(id),
            None => SessionChoice::New,
        },
        model: global.model.clone(),
        reasoning: global.reasoning.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_workspace_is_resolved_without_a_shell() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let home = root.join("home");
        let launch = root.join("launch");
        let project = home.join("Documents").join("Mon projet");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(launch.join("sub")).unwrap();
        let r = |arg: Option<&str>| resolve_workspace(&launch, arg.map(Path::new), Some(&home));
        assert_eq!(r(None).unwrap(), launch, "default: the launch folder");
        assert_eq!(r(Some("sub")).unwrap(), launch.join("sub"));
        assert_eq!(r(Some("./sub/..")).unwrap(), launch);
        assert_eq!(r(Some(project.to_str().unwrap())).unwrap(), project);
        // A quoted `~` reaches Bricks as is: expanded here, spaces kept.
        assert_eq!(r(Some("~/Documents/Mon projet")).unwrap(), project);
        assert_eq!(r(Some("~")).unwrap(), home);
        // Not expanded: `~user`, variables, a `~` that is not first.
        let e = r(Some("~bob/x")).unwrap_err();
        assert!(e.contains("~user"), "{e}");
        assert!(
            r(Some("$HOME/Documents"))
                .unwrap_err()
                .contains("does not exist")
        );
        assert!(r(Some("sub/~")).unwrap_err().contains("does not exist"));
        // No home folder: a clear error.
        let e = resolve_workspace(&launch, Some(Path::new("~/x")), None).unwrap_err();
        assert!(e.contains("home folder is unknown"), "{e}");
    }

    #[test]
    fn a_missing_folder_or_a_file_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let e = resolve_workspace(root.path(), Some(Path::new("absent")), None).unwrap_err();
        assert!(
            e.contains("does not exist") && e.contains("nothing is created"),
            "{e}"
        );
        assert!(!root.path().join("absent").exists());
        std::fs::write(root.path().join("f.txt"), "x").unwrap();
        let e = resolve_workspace(root.path(), Some(Path::new("f.txt")), None).unwrap_err();
        assert!(e.contains("is a file"), "{e}");
    }

    #[cfg(unix)]
    #[test]
    fn a_symbolic_alias_is_the_folder_it_points_to() {
        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("atlas");
        std::fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, root.path().join("alias")).unwrap();
        assert_eq!(
            resolve_workspace(root.path(), Some(Path::new("alias")), None).unwrap(),
            real.canonicalize().unwrap()
        );
    }
}

#[cfg(test)]
mod prompt_tests {
    use super::*;
    use bricks_agent::BricksConfig;
    use bricks_agent::control::scripted::{Reply, Script, ScriptedCatalog};
    use bricks_agent::control::{Command, Controller, Event, OpenOptions, Prompt, SessionChoice};
    use bricks_config::Projects;

    const REFERENCE: &str = include_str!("../../bricks-agent/tests/fixtures/bricks_foundation.txt");

    fn registry() -> Arc<ProviderRegistry> {
        Arc::new(
            ProviderRegistry::from_toml_str(
                "schema_version = 1\n\n[[providers]]\nid = \"local\"\nname = \"L\"\nendpoint = \"http://127.0.0.1:9/v1\"\nprotocol = \"chat_completions\"\nauth = \"none\"\n\n[[providers.models]]\nid = \"m\"\nname = \"M\"\napi_model = \"m\"\nstreaming = true\n\n[providers.models.limits]\nmax_input_tokens = 100000\nmax_output_tokens = 4096\n",
                "test.toml",
            )
            .unwrap(),
        )
    }

    /// The system prompt of the first request of a session opened through
    /// the real controller with the shared project loader.
    async fn sent(interactive: bool, extra: &str) -> (String, Vec<String>) {
        let dir = tempfile::tempdir().unwrap();
        let wd = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::write(
            wd.join("bricks.toml"),
            format!("[agent]\nmodel = \"test/a\"\n\n[semantic.lsp]\nenabled = false\n\n{extra}"),
        )
        .unwrap();
        let script = Script::new(vec![Reply::text("ok")]);
        let mut cfg = EngineConfig::new(
            &wd,
            ScriptedCatalog::new(&["a"], script.clone()),
            BricksConfig::default(),
            wd.join(".sessions"),
        );
        cfg.interactive = interactive;
        cfg.project_loader = Some(Arc::new(Projects::new(registry(), interactive)));
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
        while let Some(e) = events.next().await {
            if matches!(e.event, Event::RunFinished { .. }) {
                break;
            }
        }
        let req = script.requests()[0].1.clone();
        ctl.close().await;
        (
            req.system.unwrap_or_default(),
            req.tools.iter().map(|t| t.name.clone()).collect(),
        )
    }

    fn without_ask(prompt: &str) -> String {
        prompt
            .lines()
            .filter(|l| !l.starts_with("- Utilise Ask "))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn without(line_start: &str) -> String {
        REFERENCE
            .trim_end_matches('\n')
            .lines()
            .filter(|l| !l.starts_with(line_start))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tokio::test]
    async fn interactive_and_headless_send_the_same_foundation() {
        for interactive in [true, false] {
            let (s, tools) = sent(interactive, "").await;
            // `Ask` exists only when someone can answer: its line aside, the
            // foundation is the same.
            assert_eq!(tools.iter().any(|t| t == "Ask"), interactive);
            let s = without_ask(&s);
            assert!(tools.iter().any(|t| t == "Agent") && tools.iter().any(|t| t == "CodeScout"));
            assert!(s.starts_with(REFERENCE.trim_end_matches('\n')), "{s}");
            assert_eq!(s.matches("Tu es Bricks").count(), 1);
            assert!(!s.contains("Cersei SDK"));
            assert!(s.contains("<working_directory>"));

            let (s, tools) = sent(interactive, "[agents]\nenabled = false\n").await;
            let s = without_ask(&s);
            assert!(!tools.iter().any(|t| t == "Agent" || t == "Agents"));
            assert!(s.starts_with(&without("- Délègue aux sous-agents")), "{s}");
        }
    }
}
