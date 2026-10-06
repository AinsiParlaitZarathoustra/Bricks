//! `bricks run`: one prompt, no terminal interface.
//!
//! * The prompt is the positional argument; `-` (or no argument while stdin
//!   is not a terminal) reads it from stdin. `--stdin` attaches stdin after a
//!   positional prompt. A positional prompt never reads stdin otherwise.
//! * `--json`: stdout carries only versioned JSONL envelopes; everything
//!   else goes to stderr.
//! * Approvals are asked on the controlling terminal (`/dev/tty`) when
//!   there is one and `--non-interactive` is not given; otherwise a step
//!   that needs one stops the run (exit code 3).
//! * Ctrl+C cancels the run (or the memory maintenance); a second Ctrl+C
//!   exits at once.

use crate::args::{Global, RunArgs};
use crate::exit;
use crate::setup;
use cersei_agent::control::*;
use std::io::{IsTerminal, Read, Write};

fn read_stdin() -> Result<String, String> {
    let mut s = String::new();
    std::io::stdin()
        .read_to_string(&mut s)
        .map_err(|e| format!("cannot read stdin: {e}"))?;
    Ok(s)
}

/// The prompt blocks, by the rules in the module docs.
pub fn prompt_blocks(
    args: &RunArgs,
    stdin_is_terminal: bool,
    stdin: impl FnOnce() -> Result<String, String>,
) -> Result<Vec<PromptBlock>, String> {
    let mut blocks = Vec::new();
    match (args.prompt.as_deref(), args.stdin) {
        (Some("-"), _) => blocks.push(PromptBlock::Text { text: stdin()? }),
        (Some(p), false) => blocks.push(PromptBlock::Text {
            text: p.to_string(),
        }),
        (Some(p), true) => {
            if stdin_is_terminal {
                return Err("--stdin needs piped input".into());
            }
            blocks.push(PromptBlock::Text {
                text: p.to_string(),
            });
            blocks.push(PromptBlock::Text {
                text: format!("<stdin>\n{}\n</stdin>", stdin()?),
            });
        }
        (None, _) => {
            if stdin_is_terminal {
                return Err(
                    "no prompt: give one (`bricks run \"...\"`) or pipe it on stdin".into(),
                );
            }
            blocks.push(PromptBlock::Text { text: stdin()? });
        }
    }
    let has_text = blocks
        .iter()
        .any(|b| matches!(b, PromptBlock::Text { text } if !text.trim().is_empty()));
    if !has_text {
        return Err("the prompt is empty".into());
    }
    blocks.extend(
        args.files
            .iter()
            .map(|p| PromptBlock::File { path: p.clone() }),
    );
    blocks.extend(
        args.images
            .iter()
            .map(|p| PromptBlock::Image { path: p.clone() }),
    );
    Ok(blocks)
}

/// Whether a person can be asked: a controlling terminal exists.
fn controlling_terminal() -> bool {
    #[cfg(unix)]
    {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .is_ok()
    }
    #[cfg(not(unix))]
    {
        std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
    }
}

/// Ask on the controlling terminal. Anything but y/a is a refusal.
fn ask_on_terminal(req: &ApprovalRequest) -> Decision {
    #[cfg(unix)]
    let tty = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty");
    #[cfg(not(unix))]
    let tty: std::io::Result<std::fs::File> = Err(std::io::Error::other("no tty"));
    let Ok(mut tty) = tty else {
        return Decision::Deny;
    };
    let mut text = format!(
        "\n── approval: {} ({}) ──\n{}\n",
        req.tool, req.level, req.description
    );
    if let Some(p) = &req.preview {
        for f in &p.files {
            text.push_str(&format!("{} (+{} −{})\n", f.path, f.added, f.removed));
            for line in f.diff.lines().take(200) {
                text.push_str(line);
                text.push('\n');
            }
        }
    } else {
        let input = serde_json::to_string_pretty(&req.input).unwrap_or_default();
        text.push_str(&input.lines().take(40).collect::<Vec<_>>().join("\n"));
        text.push('\n');
    }
    text.push_str("Allow? [y]es / [a]lways for this session / [N]o: ");
    let _ = tty.write_all(text.as_bytes());
    let _ = tty.flush();
    let mut line = String::new();
    let mut reader = std::io::BufReader::new(tty);
    let _ = std::io::BufRead::read_line(&mut reader, &mut line);
    match line.trim().to_lowercase().as_str() {
        "y" | "yes" | "o" | "oui" => Decision::Allow,
        "a" | "always" => Decision::AllowForSession,
        _ => Decision::Deny,
    }
}

fn input_summary(input: &serde_json::Value) -> String {
    for key in ["file_path", "path", "command", "pattern", "url", "query"] {
        if let Some(v) = input.get(key).and_then(|v| v.as_str()) {
            let one: String = v.lines().next().unwrap_or("").chars().take(80).collect();
            return one;
        }
    }
    String::new()
}

/// Human-readable output: the answer on stdout, the rest on stderr.
struct Human {
    at_line_start: bool,
}

impl Human {
    fn show(&mut self, env: &Envelope) {
        let mut err = std::io::stderr();
        match &env.event {
            Event::SessionOpened { warnings, .. } => {
                for w in warnings {
                    let _ = writeln!(err, "bricks: warning: {w}");
                }
            }
            Event::TextDelta { text } => {
                print!("{text}");
                let _ = std::io::stdout().flush();
                self.at_line_start = text.ends_with('\n');
            }
            Event::ToolStarted { name, input, .. } => {
                self.newline();
                let _ = writeln!(err, "▸ {name} {}", input_summary(input));
            }
            Event::ToolFinished {
                name,
                is_error: true,
                output,
                ..
            } => {
                let first = output.lines().next().unwrap_or("");
                let _ = writeln!(err, "  ✗ {name}: {first}");
            }
            Event::EditApplied { files, .. } => {
                for f in files {
                    let _ = writeln!(err, "  ✓ {} (+{} −{})", f.path, f.added, f.removed);
                }
            }
            Event::Notice { message } => {
                let _ = writeln!(err, "· {message}");
            }
            Event::CommandRejected { reason, .. } => {
                let _ = writeln!(err, "bricks: {reason}");
            }
            Event::RunFinished {
                outcome,
                error,
                approvals_unsatisfied,
                ..
            } => {
                self.newline();
                match outcome {
                    RunOutcome::Succeeded => {}
                    RunOutcome::Incomplete => {
                        let _ = writeln!(
                            err,
                            "bricks: incomplete: {}",
                            error.clone().unwrap_or_default()
                        );
                    }
                    RunOutcome::Cancelled => {
                        let _ = writeln!(err, "bricks: cancelled");
                    }
                    RunOutcome::Failed => {
                        let _ = writeln!(
                            err,
                            "bricks: run failed: {}",
                            error.clone().unwrap_or_default()
                        );
                        for a in approvals_unsatisfied {
                            let _ = writeln!(
                                err,
                                "  needed approval: {} {}",
                                a.tool,
                                input_summary(&a.input)
                            );
                        }
                    }
                }
            }
            Event::MemoryMaintenanceFinished {
                outcome: MaintenanceOutcome::Failed,
                error,
                report,
            } => {
                let detail = error
                    .clone()
                    .or_else(|| report.as_ref().map(|r| r.errors.join("; ")))
                    .unwrap_or_default();
                let _ = writeln!(err, "bricks: long-term memory maintenance failed: {detail}");
            }
            _ => {}
        }
    }

    fn newline(&mut self) {
        if !self.at_line_start {
            println!();
            self.at_line_start = true;
        }
    }
}

pub async fn run(global: &Global, args: RunArgs) -> i32 {
    let blocks = match prompt_blocks(&args, std::io::stdin().is_terminal(), read_stdin) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("bricks: {e}");
            return exit::USAGE;
        }
    };
    let interactive = !args.non_interactive && controlling_terminal();
    let cfg = match setup::engine(global, interactive) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("bricks: {e}");
            return exit::USAGE;
        }
    };
    let (ctl, mut events) =
        match Controller::open(cfg, setup::open_options(global, args.session.clone())).await {
            Ok(x) => x,
            Err(e) => {
                eprintln!("bricks: {e}");
                return exit::USAGE;
            }
        };
    let mut human = Human {
        at_line_start: true,
    };
    let json = args.json;
    let mut stdout = std::io::stdout();
    let mut emit = |env: &Envelope| -> bool {
        if json {
            let line = serde_json::to_string(env).unwrap_or_default();
            writeln!(stdout, "{line}")
                .and_then(|_| stdout.flush())
                .is_ok()
        } else {
            human.show(env);
            true
        }
    };

    // The session is open; its first event is ready.
    if let Some(e) = events.next().await {
        if !emit(&e) {
            return exit::FAILED;
        }
    }
    if let Err(e) = ctl.send(Command::Submit {
        prompt: Prompt { blocks },
    }) {
        // Reported as `command_rejected` too.
        if let Some(env) = events.next().await {
            emit(&env);
        }
        if !json {
            eprintln!("bricks: {e}");
        }
        return exit::USAGE;
    }

    // Ctrl+C: cancel; a second one exits.
    {
        let ctl = ctl.clone();
        tokio::spawn(async move {
            let mut presses = 0;
            while tokio::signal::ctrl_c().await.is_ok() {
                presses += 1;
                if presses > 1 {
                    eprintln!("\nbricks: interrupted");
                    std::process::exit(exit::CANCELLED);
                }
                let _ = ctl.send(Command::Cancel);
            }
        });
    }

    let mut code = exit::OK;
    let mut finished = false;
    while let Some(env) = events.next().await {
        if !emit(&env) {
            // stdout is gone (closed pipe): stop the run.
            let _ = ctl.send(Command::Cancel);
            ctl.wait_idle().await;
            return exit::FAILED;
        }
        match &env.event {
            Event::ApprovalRequested { approval } if interactive => {
                let req = approval.clone();
                let decision = tokio::task::spawn_blocking(move || ask_on_terminal(&req))
                    .await
                    .unwrap_or(Decision::Deny);
                let _ = ctl.send(Command::Approve {
                    approval_id: approval.approval_id.clone(),
                    decision,
                    reason: (decision == Decision::Deny)
                        .then(|| "rejected on the terminal".to_string()),
                });
            }
            Event::RunFinished {
                outcome, failure, ..
            } => {
                finished = true;
                code = match (outcome, failure) {
                    (RunOutcome::Succeeded, _) => exit::OK,
                    (RunOutcome::Incomplete, _) => exit::INCOMPLETE,
                    (RunOutcome::Cancelled, _) => exit::CANCELLED,
                    (RunOutcome::Failed, Some(FailureKind::ApprovalRequired)) => exit::APPROVAL,
                    (RunOutcome::Failed, _) => exit::FAILED,
                };
                ctl.wait_idle().await;
                if args.no_wait_memory || !ctl.snapshot().maintenance_running {
                    break;
                }
            }
            Event::MemoryMaintenanceFinished { outcome, .. } if finished => {
                if *outcome == MaintenanceOutcome::Failed && code == exit::OK {
                    code = exit::MEMORY;
                }
                break;
            }
            _ => {}
        }
    }
    ctl.close().await;
    code
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn args(line: &[&str]) -> RunArgs {
        let cli = crate::args::Cli::parse_from([&["bricks", "run"], line].concat());
        match cli.command {
            Some(crate::args::Cmd::Run(a)) => a,
            _ => unreachable!(),
        }
    }

    fn text(b: &PromptBlock) -> &str {
        match b {
            PromptBlock::Text { text } => text,
            _ => "",
        }
    }

    #[test]
    fn positional_prompt_and_stdin_never_mix_silently() {
        let piped = || Ok("depuis stdin".to_string());
        // Positional: stdin untouched, even when piped.
        let b = prompt_blocks(&args(&["Analyse"]), false, || panic!("stdin read")).unwrap();
        assert_eq!(text(&b[0]), "Analyse");
        // `-` and no argument read stdin.
        assert_eq!(
            text(&prompt_blocks(&args(&["-"]), false, piped).unwrap()[0]),
            "depuis stdin"
        );
        assert_eq!(
            text(&prompt_blocks(&args(&[]), false, piped).unwrap()[0]),
            "depuis stdin"
        );
        // No argument on a terminal is an error, not a wait.
        assert!(prompt_blocks(&args(&[]), true, piped).is_err());
        // --stdin: both, in two blocks.
        let b = prompt_blocks(&args(&["Résume", "--stdin"]), false, piped).unwrap();
        assert_eq!(b.len(), 2);
        assert!(text(&b[1]).contains("depuis stdin"));
        assert!(prompt_blocks(&args(&["x", "--stdin"]), true, piped).is_err());
        assert!(
            prompt_blocks(&args(&["-"]), false, || Ok("  ".into())).is_err(),
            "empty"
        );
        let b = prompt_blocks(
            &args(&["x", "--file", "a b.rs", "--image", "i.png"]),
            true,
            piped,
        )
        .unwrap();
        assert_eq!(
            b[1],
            PromptBlock::File {
                path: "a b.rs".into()
            }
        );
        assert_eq!(
            b[2],
            PromptBlock::Image {
                path: "i.png".into()
            }
        );
    }
}
