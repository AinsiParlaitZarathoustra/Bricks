//! PowerShell tool: commands in this session's persistent PowerShell
//! (`pwsh`, or Windows PowerShell). `$env:…`, `Set-Location`, variables,
//! functions and aliases persist while the session lives.
//!
//! The result separates PowerShell's own success, cmdlet errors and the exit
//! code of a native program — which is reported only when one ran in that
//! command. A timeout replaces the session (reported); see
//! `crate::shell::powershell` and `docs/shell.md`.

use super::*;
use crate::shell::{self, powershell::PwshStatus};
use crate::tool_report::{ToolBody, ToolReport, ToolStatus};
use serde::Deserialize;
use std::time::Duration;

pub struct PowerShellTool;

#[async_trait]
impl Tool for PowerShellTool {
    fn name(&self) -> &str {
        "PowerShell"
    }
    fn description(&self) -> &str {
        "Run a command in this session's persistent PowerShell ($env:, Set-Location, variables, \
         functions and aliases persist). Available on Windows; on macOS/Linux uses pwsh if \
         installed. `timeout` in milliseconds (default 120000)."
    }
    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Execute
    }
    fn category(&self) -> ToolCategory {
        ToolCategory::Shell
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "PowerShell command to execute" },
                "timeout": { "type": "integer", "description": "Timeout in milliseconds (default 120000)" }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            command: String,
            timeout: Option<u64>,
        }
        let input: Input = match crate::tool_feedback::parse_input(self, &input) {
            Ok(i) => i,
            Err(e) => return e,
        };
        let config = crate::bash::shell_config(ctx);
        let timeout = input
            .timeout
            .map(Duration::from_millis)
            .unwrap_or(config.default_timeout)
            .min(config.max_timeout);
        let started = std::time::Instant::now();
        let sessions = shell::session(&ctx.session_id, &config);
        let session = match sessions.pwsh(&ctx.working_dir, &config).await {
            Ok(s) => s,
            Err(e) => {
                let mut r = ToolReport::new(
                    ToolStatus::Failure,
                    ToolBody::Streams {
                        stdout: String::new(),
                        stderr: String::new(),
                    },
                );
                r.termination = Some(format!("lancement impossible : {e}"));
                r.duration = Some(started.elapsed());
                if e.kind() == std::io::ErrorKind::NotFound {
                    r.suggestion = Some(if cfg!(windows) {
                        "Vérifiez que PowerShell est disponible dans le PATH.".into()
                    } else {
                        "Installez PowerShell (pwsh) ou utilisez l'outil Bash.".into()
                    });
                }
                return ToolResult::from_report(r);
            }
        };
        let reset = sessions.take_reset_notice();
        let outcome = match session
            .run(
                &input.command,
                timeout,
                crate::bash::progress_for(ctx, "PowerShell"),
            )
            .await
        {
            Ok(o) => o,
            Err(e) => return ToolResult::error(format!("PowerShell session failed: {e}")),
        };
        let mut report = ToolReport::new(
            ToolStatus::Success,
            ToolBody::Streams {
                stdout: outcome.stdout.text.clone(),
                stderr: outcome.stderr.text.clone(),
            },
        );
        match outcome.status {
            PwshStatus::Completed {
                ok,
                cmdlet_errors,
                native_exit,
            } => {
                let failed = !ok || cmdlet_errors > 0 || native_exit.is_some_and(|c| c != 0);
                report.status = if failed {
                    ToolStatus::Failure
                } else {
                    ToolStatus::Success
                };
                report.exit_code = native_exit;
                if native_exit.is_none() {
                    report.termination = Some(format!(
                        "pas de programme natif ; PowerShell {} ({} erreur(s) de cmdlet)",
                        if ok { "a réussi" } else { "a échoué" },
                        cmdlet_errors
                    ));
                } else if cmdlet_errors > 0 {
                    report.notes.push(format!(
                        "{cmdlet_errors} erreur(s) de cmdlet en plus du programme natif"
                    ));
                }
            }
            PwshStatus::TimedOut => {
                report.status = ToolStatus::TimedOut;
                report.timeout = Some(timeout);
                report.termination =
                    Some("aucun code de sortie (session PowerShell détruite)".into());
            }
            PwshStatus::SessionEnded { code } => {
                report.status = ToolStatus::Failure;
                report.exit_code = code;
                if code.is_none() {
                    report.termination = Some("session PowerShell terminée sans statut".into());
                }
            }
        }
        report.duration = Some(outcome.duration);
        if let Some(r) = reset {
            report.notes.push(r);
        }
        report.notes.extend(outcome.notes);
        report.data = Some(serde_json::json!({ "cwd": outcome.cwd }));
        ToolResult::from_report(report)
    }
}
