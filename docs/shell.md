# Shell, file tools and tool results

How `Bash`, `PowerShell` and `Job` work, how `Read`, `Edit` and `ApplyPatch` behave, and what a tool result looks like.

## 1. Bash

**Every `Bash` call is its own `bash` process.** Commands of one agent run side by side. A long command does not hold back a short one. All the tools of one agent share one shell state and one set of background jobs. Another agent, a sub-agent included, has its own.

**What a command starts from.** It gets a copy of the session state: the current directory, exported variables, functions and aliases. Bricks' own environment is never changed.

**What a command passes on.** When a command ends normally, only the changes it made are saved for the next commands:

* the working directory;
* exported variables added, changed or removed;
* functions and aliases added, changed or removed.

Rules:

* If several commands change the same thing, the one that was **started last** wins, whatever the order they end in.
* A non-zero exit code does not drop the changes. A command that failed after a `cd` still keeps the `cd`.
* `exit` saves the changes. `exec` does not.
* A command that was interrupted, timed out, was killed, or replaced the EXIT trap saves nothing. The result says `environment changes not available`.
* `background: true` jobs start from a copy and never save changes (see section 3).

**What does not carry over:** shell variables that are not exported, shell options (`set`, `shopt`, so `set -e` is not kept), and anything done after an interruption. A `source .venv/bin/activate` works because it exports variables. nvm and conda work when they export what they need.

### How a command is run (Unix)

* One `bash --noprofile --norc` per call. `BASH_ENV`, `ENV` and `PROMPT_COMMAND` are removed.
* Each command has its own session and process group and no terminal. A program that asks for input on the terminal (sudo, ssh) fails instead of hanging.
* Stdout and stderr are read separately. Stdin is empty unless you give `input`.
* The command text is passed as a file, so quotes, newlines, here-documents and Unicode stay exact.
* Status and final state come from the process, never from its output.
* Names that start with `__bricks_` are reserved.

### Working directories

| Tool | Relative paths start from |
|---|---|
| `Bash` | The command's own current directory. It starts from the session's directory and is shown in each result as `cwd`. |
| `PowerShell` | The PowerShell session's location (`Set-Location`). |
| `Read`, `Edit`, `Write`, `ApplyPatch`, `Glob`, `Grep`, `CodeStats` | The agent's working directory. A `cd` in a shell never moves it. |

### Permissions

The session's permission policy decides before the command runs, from the command text. If the command uses an alias or function defined in the session, its definition is added to the approval request. So an alias such as `tidy='rm -rf ...'` cannot hide what will run.

## 2. Running, timeouts and cleanup

| `Bash` input | Default | Meaning |
|---|---|---|
| `command` | required | The command. |
| `timeout` | 120000 ms (max 600000) | Deadline from the start. It still applies after a hand-over. |
| `yield_time_ms` | `[bash] yield_time_ms`, 10000 | Foreground only. How long the call waits before a still-running command is handed over as a job. It is not a deadline. |
| `input` | none | Text for stdin. Prompts are never answered for you. |
| `background` | `false` | Start a job at once (section 3). |
| `ready_pattern`, `ready_timeout` | none, 10 s | Background only. A regex on output lines that means "ready", and how long to wait for it. |

A command that may become a job (`background: true`, or a `yield_time_ms` below its `timeout`) needs the session's job registry. Without one (for example `bricks_tools::shell()` alone), it is refused before it starts. To make the call wait for the end, set `yield_time_ms` at least equal to `timeout`.

Other defaults are in `ShellConfig` (`default_timeout`, `max_timeout`, `yield_time`, `term_grace` 2 s, `drain` 500 ms, `progress_every` 10 s, capture and log limits, `base_dir`, `bash_path`, `pwsh_path`). The `[bash]` section of `bricks.toml` only sets `yield_time_ms` (see `docs/bricks.example.toml`).

**Progress.** While a command runs, a progress message is sent every `progress_every`: elapsed time, bytes so far, and a hint when nothing was printed for a while.

**Timeout.** The command's whole process tree gets `SIGTERM`, then `SIGKILL` after `term_grace`. Bricks checks that the processes are gone. The command saves no state. The result has status `timed_out`, no exit code, the partial output and the number of processes stopped. Background jobs are not touched.

**Yield.** A foreground command still running after `yield_time_ms` is not killed. The call returns status `Running` with the job id and read positions, and the command goes on until it ends or its `timeout` expires.

**Leftovers.** When a foreground command ends, any process it left behind (`cmd &`, a daemon) is stopped and listed in the result. A process that leaves its group on purpose (`setsid`, a double-forked daemon) is not found and survives. To keep something running, use `background: true`.

**Cancel.** Cancelling the agent's turn interrupts every ordinary command at once. Background jobs survive the turn, but not the session.

**Output.** Bricks removes ANSI escape sequences, carriage-return progress and control characters. It keeps up to 4 MiB of raw output per stream in memory. Beyond that, the full raw stream goes to a file and its path is in the result. The cleaned view keeps the first 1 MiB and the last 1 MiB and says what was skipped. Output compression (`docs/compression.md`) runs after, on the cleaned text only.

**Closing.** `Agent::close().await` stops the session's jobs and commands. Dropping the `Agent` does the same with forced signals.

### Cleanup by system

| Situation | Unix (macOS, Linux) | Windows |
|---|---|---|
| Close or drop | All trees are ended. | `taskkill /T /F` on each tree. |
| Timeout or stop of a foreground command | Descendants are found by parent links, TERM then KILL, and checked. | The PowerShell session is destroyed with its tree. |
| A daemon that detached itself | Not found. | Not found. |
| Bricks killed with SIGKILL or a crash | Nothing cleans up. Commands and jobs survive. | Same. |

## 3. Yielded commands and background jobs

A `Bash` call can return while the command still runs, in two ways:

* **At once**, with `background: true`. The call returns the `job_id` (`job_` and 12 hex characters), pid, state, readiness and directory. If the job already ended, or `ready_pattern` was not seen in time, the last 50 lines of output follow.
* **After `yield_time_ms`**, for a foreground command. The call returns status `Running` with the same details and the stdout and stderr positions. The command keeps its `timeout`. Unlike a background job, it is stopped when the run that started it ends. `Job` with `wait` lets it finish first.

Both are jobs: a separate `bash` that starts from a snapshot of the session state. A job never changes the session state. It has its own pipes, so its output never mixes with other commands.

* **States:** `running`, `completed` (exit 0), `failed` (non-zero exit, or a signal Bricks did not send), `stopped`.
* **Ready is not working.** With `ready_pattern`, readiness is reported when a line matches (for example `port \d+`). Without it, the result says readiness was not checked.
* **Limit:** `[background] max_jobs` (default 16) running jobs per session. Beyond it, the start is refused. There is no queue.
* **Owner:** a job is visible and stoppable only by the agent that started it. A frontend sees every job of its session.
* **Lifetime:** a background job survives its turn and ends with the session. A yielded command ends with the run that started it. A sub-agent's jobs stop when it ends.
* **Kept output:** per stream, 1 MiB in memory (`output_buffer_bytes`) and at most 5000 lines. A line over 64 KiB (`max_line_bytes`) is cut and marked. Raw logs are capped at 50 MiB per stream (`raw_log_bytes`) and copied to the session's files when the job ends.

### The `Job` tool

| Input | Default | Rule |
|---|---|---|
| `action` | `list` | `list`, `status`, `output`, `wait` or `stop`. |
| `job_id` | none | Required for every action except `list`. |
| `stream` | `stdout` | `stdout` or `stderr`, for `output`. |
| `offset` | `0` | First line to read (from 0), for `output`. |
| `limit` | `200` | 1 to 2000 lines, for `output`. |
| `tail` | `false` | Read the last `limit` lines, for `output`. Not with `offset`. |
| `timeout_ms` | `60000` | 1 to 600000, for `wait`. |

Unknown fields and out-of-range values are refused, never adjusted. `stop` needs Execute permission. The other actions are read-only.

* `list`: one line per job (id, state, pid, duration, readiness, output sizes, directory, command).
* `status`: the same line, then the raw log paths.
* `output`: a header (stream, range, totals, next offset), then the lines. Pages are at most `max_page_bytes` (64 KiB).
* `wait`: returns when the job ends or after `timeout_ms`. A timeout stops nothing. When the job has ended, the last 100 lines of each stream follow.
* `stop`: TERM to the group and its tree, a grace period (`stop_grace_ms`, 2 s), then KILL. Stopping an ended job is a success.
* An unknown id, or another agent's, gets: `No accessible job with this id.`

Prefer `background: true` or a yield to `cmd &` in a foreground command.

## 4. PowerShell

One `pwsh` (or `powershell` on Windows) per session, started with `-NoProfile -NonInteractive` and empty stdin. Variables, functions, aliases, `$env:` and `Set-Location` persist. Requests run one after another (unlike `Bash`). Control and output travel on an authenticated local TCP connection.

The result separates PowerShell's success (`$?`), the number of cmdlet errors, and the exit code of a native program (only when one ran in that request). A timeout, `exit`, a crash or a lost connection ends the session. The next command starts a new one and says so.

## 5. Edit

`Edit` makes one write per call. A call is one replacement or several:

| Form | Input |
|---|---|
| One replacement | `{file_path, old_string, new_string?, replace_all?}` |
| Several replacements | `{file_path, edits: [{old_string, new_string?, replace_all?}, ...]}` |

* The two forms cannot be mixed. `edits` must not be empty. Unknown keys are refused and named.
* A missing or empty `new_string` deletes the match.
* Several replacements are applied in order, in memory, each on the result of the previous one. The file is written only if all of them apply, in one atomic write. A failure names the edit (`edit N of M`) and writes nothing.
* A call that would change nothing is refused.
* `Edit` does not require a `Read` first.

How the text is matched (the first step with one clear match wins):

1. **Exact.** Several identical matches are refused unless `replace_all` is set.
2. **Normalized**, line by line: line endings, trailing spaces, and one constant indentation shift for the whole block. Relative indentation must match, which protects Python. Lines inside multi-line strings and here-documents must match exactly.
3. **Moved block** (files up to 100000 lines, blocks up to 400 lines): the same lines, where only blank lines and runs of spaces outside quotes may differ. It never accepts different code.

The replacement follows the file's indentation and line endings. Bytes outside the block are untouched. When a match fails, the message says why (absent, ambiguous, or the code itself differs) and shows the closest places.

**Concurrency.** One lock per file, shared by every session and agent of the Bricks process, is held from the read to the write. Two edits of one file run one after the other. No lock is held while an approval is pending. Bricks also checks that the file did not change on disk between its read and its write, and refuses the call if it did.

**Approval.** The preview and the written text come from the same preparation. The approval records a SHA-256 of the text the diff was computed from. Just before the write, Bricks checks it again. If the file changed since, nothing is written and the call is decided again on the current file. A hook that changes the input is decided again if its change differs from the approved one.

`MultiEdit` does not exist. Use `Edit` with `edits`. In Rust, use `bricks_tools::file_edit::FileEditTool`.

## 6. ApplyPatch

Takes a unified diff (`diff -u`, `git diff`): several files and hunks, creating and deleting files through `/dev/null`, `a/` and `b/` prefixes, CRLF files, and `\ No newline at end of file`. Hunk line counts are checked. Context and removed lines must match the file exactly.

Refused: binary patches, renames, copies, mode changes, and a file that appears twice. Paths must be relative, stay inside the working directory and cross no symlink. Everything is checked before anything is written. Files are then written one by one, atomically. If a write fails, the files already written are restored, and any that could not be restored are named. It is not a multi-file transaction.

## 7. Read

Lines are shown as `  12 | text`, numbered from 1. A page has `limit` lines (default 2000), starting at `offset` (1-based line, default 1). The result gives the total and the next offset. A file that changes during the read is flagged.

* UTF-8 text is accepted, with or without a BOM. Lines longer than 4000 characters are cut and marked.
* Binary files are reported with their size and probable type, never shown. UTF-16 and other encodings are reported as unsupported.
* **Images** (PNG, JPEG, GIF, WebP, up to 3.75 MB) are shown to the model. `offset` and `limit` do not apply to images.
* Relative paths start from the agent's working directory.

Reading a file before changing it is a good idea, but no tool requires it.

## 8. Tool results

Every tool result has data and one text rendering. The report holds the status (`success`, `failure`, `timed_out`, `cancelled`, `running`), the exit code when a process exited normally (never invented), the cause of the end, the duration, stdout and stderr, a suggestion for known errors, notes and structured data. The text looks like this (the real header separates the parts with a long dash):

```text
✓ [Bash] Success (0.42s) - code 0
--- stdout ---
...
```

```text
✗ [Bash] Failed (1.15s) - code 127
--- stderr ---
bash: foobar: command not found
--- suggestion ---
Check that the tool is installed and available on PATH.
```

```text
⏱ [Bash] Interrupted after timeout of 120s (121.40s)
[Interrupted after timeout of 120s - partial output below]
--- stdout ---
...
```

A successful command with no output reads `(Command ran successfully with no output)`. A yielded or background command reads `... [Bash] Running`, with its `job_id`, pid and positions. The header describes the call. It is not output, and compression never sees it. Notes (`--- notes ---`) and the suggestion come after the output.

## 9. Platforms tested

| Platform | What ran |
|---|---|
| macOS (arm64), bash 3.2.57 | Shell tests (`crates/bricks-tools/tests/shell_session.rs`), agent tests (`crates/bricks-agent/tests/shell_tools_e2e.rs`), file-tool tests. |
| Linux | Not run. The Unix code path is the same as macOS. |
| Windows, PowerShell | Not run and not built. `crates/bricks-tools/tests/powershell_session.rs` is `#[ignore]` and needs `--ignored` where PowerShell exists. Windows Job Objects are not used (trees end with `taskkill /T /F`). `Bash` is not available on Windows. |
