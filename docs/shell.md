# Shell, file tools and tool results

Contracts of the `Bash`, `PowerShell` and `Job` tools (background jobs), of
`Edit`, `ApplyPatch` and `Read`, and of the result format every tool shares.

## 1. Bash: one process per command, shared state

**Every `Bash` call is its own `bash` process.** Commands of one agent session
run side by side: none waits for another, and a long command does not hold
back a short one. An agent's tools share one session id (its `session_id` when
set, else one generated per agent), hence one state and one set of background
tasks. Another agent, a sub-agent included, has its own.

**What a command starts from.** At launch it gets a copy of the session state:
the current directory, the exported variables, the functions and the aliases
(restored by a prelude sourced before the command). The process environment of
Bricks itself is never modified.

**What a command publishes.** When it ends normally, its shell records its
final state and Bricks publishes the **delta** against its launch copy only:

* the working directory (`PWD` and `OLDPWD` move together);
* exported variables added, changed or removed;
* functions and aliases added, changed or removed.

Rules for the published state:

* For one field, the command that was **launched most recently** among those
  that publish wins, whatever the order of their ends. An older slow command
  that ends late restores nothing.
* A non-zero exit code does not discard the changes: a command that failed
  after a `cd` or an `export` still publishes them.
* `exit` publishes; `exec` does not.
* An interrupted command, a timed-out one, one that replaced the EXIT trap
  (the recording trap) or one that was killed publishes nothing. The result
  says `environment changes not available`.
* `background: true` tasks start from a copy and never publish (§3).

**What does not persist**: non-exported shell variables, shell options
(`set`, `shopt`), and what a command changes after it was interrupted. A
`deactivate` of a Python virtual environment does not restore the `PATH`
it had set. A `source .venv/bin/activate` persists through the exports it
makes. nvm and conda persist when their setup exports what they need.

Interactions to know: `set -e` is not kept (each command has its own shell
options); a user `trap … EXIT` replaces the recording trap, so that command
publishes nothing; a process that detaches from the command's group (a daemon
that double-forks) escapes the group cleanup (§2).

### How a command is run (Unix)

* One `bash --noprofile --norc` per call (no user profile; `BASH_ENV`, `ENV`
  and `PROMPT_COMMAND` are removed), in its own session (`setsid`) and process
  group, without a controlling terminal: a program that would prompt on
  `/dev/tty` (sudo, ssh) fails instead of hanging.
* Stdout and stderr are separate pipes, read continuously. stdin is
  `/dev/null` unless `input` is given.
* The command's text is passed to the shell as a file and sourced in its
  scope: quotes, newlines, here-documents and unicode are preserved exactly.
  `return N` ends the command with status N.
* Status and final state come from the process, never from its output: output
  that imitates a marker is just output.
* Names starting with `__bricks_` are reserved.

### Working directories

| Tool | Relative paths resolved against |
| --- | --- |
| `Bash` | the command's own current directory, which starts from the session's directory and is reported in each result's data as `cwd` |
| `PowerShell` | the PowerShell session's location (`Set-Location`) |
| `Read`, `Edit`, `Write`, `ApplyPatch`, `Glob`, `Grep`, `CodeStats` | the agent's working directory: a `cd` in a shell never moves them |

A `cd` in the shell does not change the agent's working directory nor the
permission root of the workspace.

### Permissions

The session's permission policy decides before execution, on the command text. In addition, when
the command invokes an alias or function defined in the session, its
definition is appended to the request's `description`, so session state cannot
hide what will actually run (the approval shows `alias tidy='rm -rf …'` for a
call that would run it).

## 2. Execution, timeouts, supervision

| Option (Bash input) | Default | Meaning |
| --- | --- | --- |
| `timeout` | 120 000 ms (max 600 000) | Deadline from the start of the command, enforced also after it was handed over (see `yield_time_ms`). |
| `yield_time_ms` | `[bash] yield_time_ms`, 10 000 ms | Foreground only. Time the call waits before a command still running is handed over as a job (§3). Not a deadline: `timeout` is. |
| `input` | none (empty stdin) | Text given on stdin. Prompts are never answered automatically. |
| `background` | `false` | Start a supervised task at once (§3). |
| `ready_pattern` / `ready_timeout` | — / 10 s | Background only: readiness regex on output lines, and how long to wait for it. |

A command that may become a job (`background: true`, or a `yield_time_ms` below
its `timeout`) needs the session's job registry (§3). Without one, as with
`bricks_tools::shell()`, it is refused before anything starts. Give
`yield_time_ms` at least the `timeout` to have the call wait for the end.

`[bash] yield_time_ms` in `bricks.toml` sets the default (see
`docs/bricks.example.toml`). `ShellConfig` (put in `ToolContext::extensions`,
e.g. via `AgentBuilder::extensions`) sets the other defaults: `default_timeout`,
`max_timeout`, `yield_time`, `term_grace` (2 s), `drain` (500 ms),
`progress_every` (10 s), capture limits, task log limits, `base_dir`,
`bash_path`, `pwsh_path`.

**Progress.** While a command runs, a progress message is emitted every
`progress_every` (`AgentEvent::ToolProgress`): elapsed time, bytes so far,
and, when nothing was printed for a while, that the command may be computing
or waiting for input it will not get. This is a hint, not a diagnosis.

**Timeout.** An event is emitted; the command's process tree (every live
descendant, found by parent links) gets `SIGTERM`, then `SIGKILL` after
`term_grace`; termination is verified (a process is reported stopped only when
gone or a zombie). The command publishes nothing. The result has status
`timed_out`, no exit code, the partial output and the number of processes
stopped (and forced). Background tasks are not touched.

**Waiting for exits.** Stopping a tree does not poll. A watcher thread waits
in the kernel for the exit of each process it targets: `kqueue` with
`EVFILT_PROC`/`NOTE_EXIT` on macOS, a `pidfd` per process in an `epoll` set on
Linux. Each exit is written to a pipe that the waiting task reads, under the
same grace and forced-stop deadlines as before. Where no notification is
available (the kernel refuses `pidfd_open`, the platform has none, or the
polling mode is set, as in tests), the wait checks `kill(pid, 0)` every 25 ms
within the same deadline. That fallback counts a zombie as running until its
parent reaps it. On Windows, the wait still checks `tasklist` every 25 ms.

**Yield.** A foreground command still running after `yield_time_ms` is not
killed. The call returns status `Running` with its ids and cursors (§3), and
the command goes on until it ends or its `timeout` expires.

**Leftovers.** When a foreground command ends, any process it left running
(`cmd &`, a daemon that kept the outputs) is stopped and listed in the result,
with a pointer to `background: true`. Unknown descendants never keep writing
into the streams of a finished command. A process that leaves the group on
purpose (`setsid`, a double-forked daemon re-parented to init) is not found:
it survives this cleanup.

**Cancellation.** Cancelling the agent's turn drops the running tool call;
each ordinary command still active is interrupted at once (its group, then
its tree). Background jobs (`background: true`) keep their own rule: they
survive the turn, not the session (§3). An individual task stopped with
`Job` `stop` is terminated as a group and reaped.

**Output.** Bytes are cleaned incrementally (ANSI CSI/OSC/DCS/APC sequences
and their 8-bit forms, carriage-return progress, backspace, control
characters), across chunk boundaries, including escapes and UTF-8 characters
split between reads. Raw bytes are kept in memory up to 4 MiB per stream, then
the complete raw stream is spilled to a file whose path is in the result; the
cleaned view keeps 1 MiB of head and 1 MiB of tail and states the gap. Output
compression (`docs/compression.md`) runs afterwards, on the cleaned text only.

**Closing.** `Agent::close().await` stops the session's tasks and commands
(graceful, bounded, awaited); dropping the `Agent` does the same synchronously
with forced signals.

### Termination guarantees by OS

| Situation | Unix (macOS, Linux) | Windows |
| --- | --- | --- |
| Normal close / drop | Commands and tasks: whole trees terminated and reaped. | `taskkill /T /F` on each tree. |
| Timeout or stop of a foreground command | Descendants found by parent links (`ps`), TERM then KILL, exits awaited by kernel notification (`kqueue` or `pidfd`), verified. | Not stopped individually: the PowerShell session is destroyed with its tree. |
| Process that double-forks and is re-parented to init (a deliberately detached daemon) | Not found: no longer a descendant. | Not found. |
| Bricks killed with SIGKILL / crash | Nothing runs to clean up: commands and tasks survive in their own sessions. | Same. |

Process groups alone do not contain every descendant (`setsid` escapes a
group); Bricks follows parent links instead, with the limits above. No
administrator privilege is required. Job Objects are not used yet on Windows
(see §9).

## 3. Yielded commands and background jobs

A `Bash` call returns one of two ways when a command is still running:

* **At once**, with `background: true`: the call returns as soon as the job is
  started, with its `job_id` (`job_` followed by 12 hexadecimal characters), its
  pid, its state, its readiness and its working directory.
* **After `yield_time_ms`**, for a foreground command that is still running: the
  call returns status `Running` with its `job_id`, pid, state, working directory
  and the stdout and stderr cursors (the line counts to read from). The command
  is not restarted and not killed; its `timeout` still applies.

Both are jobs: a separate `bash` started from a **snapshot** of the session state
taken at launch (§1). A job never publishes into the session, so what it changes
never reaches the next commands. It runs in its own session and process group,
with its own pipes: it never blocks the next commands and its output never mixes
with theirs.

* **States**: `running`, `completed` (exit 0), `failed` (non-zero exit, or a
  signal Bricks did not send), `stopped`. The exit code exists only after a
  normal exit.
* **Ready is not running**: with `ready_pattern`, readiness is reported when a
  line matches (e.g. `port \d+`); without it, the result says readiness was not
  checked. Ready is still not proof that the service works.
* **Tool `Job`**: one action per call, on the jobs of the calling agent (see
  below). Its `job_id` comes from `Bash`; `docs/agents.md` describes the
  sub-agent case.
* **Quota**: `[background] max_jobs` (default 16) counts the running jobs of the
  session and the places being started. A place is reserved atomically before
  anything starts. Beyond the limit the start is refused with a clear message
  and nothing is launched: there is no queue.
* **Ownership**: a job is visible and stoppable only to the agent that started
  it. Another agent's id gets the same answer as an unknown id. A frontend sees
  every job of its session.
* **Lifetime**: a background job (`background: true`) survives the turn that
  started it and ends with the session. A yielded command is not durable: it
  ends with the run that started it. A sub-agent's jobs stop when it ends.
* **Bounded capture**: per stream, 5 000 lines and 1 MiB
  (`[background] output_buffer_bytes`) are kept in memory; a line longer than
  64 KiB (`max_line_bytes`) is cut and marked. Raw logs are capped at 50 MiB per
  stream (`raw_log_bytes`) in the session directory, and copied to the session's
  files when the job ends.

### The `Job` tool

| Input | Default | Rule |
| --- | --- | --- |
| `action` | `list` | `list`, `status`, `output`, `wait` or `stop`. `{}` lists the jobs. |
| `job_id` | — | Required for every action but `list`. |
| `stream` | `stdout` | `stdout` or `stderr`, for `output`. |
| `offset` | `0` | Line to start at, 0-based, for `output`. |
| `limit` | `200` | 1 to 2000 lines, for `output`. |
| `tail` | `false` | The last `limit` lines, for `output`. Not with `offset`. |
| `timeout_ms` | `60000` | 1 to 600000, for `wait`. |

Unknown fields are refused, and out-of-range values are refused, never
clamped. Permission: `stop` is Execute; `list`, `status`, `output` and `wait` are
ReadOnly.

* **`list`**: one line per job: id, state (with the exit code or signal once
  ended), pid, duration, readiness (not checked, not yet, or yes after a
  delay), lines and bytes per stream, working directory and command.
* **`status`**: the same line, then the raw log paths: the live logs while the
  job runs, the durable copies once they are kept.
* **`output`**: a header, then the lines, without numbers. The header gives the
  stream, the range returned, the complete lines so far, the bytes received,
  how many earlier lines are no longer in memory, the raw log path (with its cap
  or write error when relevant) and the next offset to read from. A first line
  longer than the page (`[background] max_page_bytes`, 64 KiB by default) is
  shown cut and marked, and the offset moves past it. A line still in progress
  is shown apart and marked; it does not move the offset.
* **`wait`**: returns when the job ends or after `timeout_ms`. A timeout says
  the job is still running and stops nothing.
* **`stop`**: TERM to the process group and its tree, a grace period
  (`stop_grace_ms`, 2 s by default), then KILL. Idempotent: a job that already
  ended is a success, and the answer gives its state.
* **Unknown id, or another agent's**: `No accessible job with this id. A job is
  visible only to the agent that started it. Use Job with action list to see your
  jobs.`
* **No jobs**: `You have no jobs.` with a pointer to `background: true` and
  `yield_time_ms`.

The `Bash` result of a job names the `Job` call that follows it.

Prefer `background: true`, or a yield, to `cmd &` in a foreground command
(whose leftovers are stopped when the command ends).

## 4. PowerShell

One `pwsh` (or `powershell` on Windows) per session, started with
`-NoProfile -NonInteractive`, its stdin empty. Each request is dot-sourced at
the driver's level, so `$env:…`, `Set-Location`, variables, functions and
aliases persist. Requests run one after another in that session (unlike
`Bash`, which runs commands side by side). Control and output travel on an
authenticated loopback TCP connection (JSON lines), never through the process's
stdin/stdout.

The result separates PowerShell's success (`$?`), cmdlet errors (counted), and
the exit code of a native program, reported only when one ran in that request
(`$LASTEXITCODE` is reset before each request, so an old code is never
recycled). A timeout, `exit`, a crash or a lost connection ends the session
with its tree; the next command starts a new one (reported). Output objects are
formatted one by one with `Out-String`.

## 5. Edit

`Edit` makes one write per call. A call is either one replacement or several:

| Form | Input |
| --- | --- |
| One replacement | `{file_path, old_string, new_string?, replace_all?}` |
| Several replacements | `{file_path, edits: [{old_string, new_string?, replace_all?}, …]}` |

* The two forms cannot be mixed. `edits` is a non-empty list. Unknown keys are
  refused and named, with the index of the entry for an `edits` item.
* An omitted or empty `new_string` deletes the match.
* Several replacements are applied **in order, in memory**, each against the
  result of the previous one. The file is written **only if every replacement
  applies** (all or nothing), through one atomic write (temporary file and
  rename, permissions kept). A failure names the edit (`edit N of M`) and
  nothing is written.
* A call that would change nothing is refused.
* Matching (first stage that finds a unique match wins):
  1. **Exact.** Several identical occurrences are refused unless `replace_all`.
  2. **Normalised**, line by line, with explicit rules only: LF/CRLF, trailing
     whitespace, one constant indentation shift for the whole block (tabs or
     spaces, tab = 4, 8 or 2 columns). Relative indentation must match, which
     protects Python. Lines inside multi-line strings, template literals, Rust
     raw strings and here-documents must match exactly. A single-line
     `old_string` also matches its trimmed text when that text occurs once in the
     file (`crates/bricks-tools/src/tool_primitives/replace.rs`).
  3. **Relocation** (bounded: files ≤ 100 000 lines, blocks ≤ 400 lines): the
     same lines allowing only blank lines and runs of spaces outside quoted text
     to differ, with the same constant indentation shift. It finds a block whose
     layout drifted; it never accepts different code, however similar.
* The replacement is re-indented in the block's style and follows the file's
  line endings; bytes outside the block are untouched.
* On refusal, the message says why (absent, ambiguous, or "the code itself
  differs") and gives the closest regions (line ranges, an excerpt, similarity).

**Concurrency.** A per-file lock, shared by every session and agent of the
Bricks process (a symbolic link and its target share one lock), is held from
the read of the file to the write, so two calls of Bricks that edit one file
run one after the other and never overwrite each other's change. No lock is
held while an approval is pending. The lock does not
arbitrate external programs (an editor, a script): Bricks still checks that the
file did not change on disk between its read and its write, and refuses the
call if it did. Edit and Read calls in the same batch have no guaranteed order.

**Approval.** The preview shown for an approval and the text the execution
writes come from the same preparation. The approval records the SHA-256 of
the text the diff was computed from. Execution checks it again under the lock,
just before the write: a file that changed since the approval is not written,
and the call is decided again on the file as it is now. A hook that changes the
input (`HookAction::ModifyInput`) is decided again when the change it makes
differs from the approved one, so the change written is the change approved.
Nothing is written without a decision.

**Migration from `MultiEdit`.** `MultiEdit` is removed; an unknown tool name is
an error. Use `Edit` with `edits`. SDK code uses
`bricks_tools::file_edit::FileEditTool` with the same `edits` form.

## 6. ApplyPatch

Unified diffs (`diff -u`, `git diff`): several files and hunks, creation and
deletion through `/dev/null`, `a/`/`b/` prefixes, timestamps after a tab, C-quoted
paths and plain paths with spaces, CRLF files, `\ No newline at end of file`.
Hunk line counts are checked; context and removed lines must match the file
exactly (at the stated line, or the nearest place after the previous hunk).
Refused explicitly: binary patches, renames, copies, mode changes, a file
appearing twice. Paths must be relative, stay inside the working directory and
go through no symlink. Everything is validated before anything is written; files
are then written atomically one by one, and if a write fails the files already
written are restored (and any that could not be is named). This is not a
multi-file transaction.

## 7. Read

`  12 | text` lines, numbered from 1 and aligned. Pages of `limit` lines (default
2000) from `offset`; the window is stated against the exact total, counted by
streaming through the file (memory bounded by the page), with the next offset. A
file that changes during the read is flagged. Binary files are reported with
their size and probable type, never shown; UTF-16 and non-UTF-8 text (Latin-1…)
are reported as unsupported encodings. Accepted: UTF-8, with or without a BOM
(the BOM is not shown); CRLF is reported in the data. Lines longer than 4000
characters are cut with a marker. Relative paths are resolved against the
agent's working directory.

Reading a file before changing it is recommended when the change needs its
exact text, but no tool requires it: `Edit`, `Write` and `ApplyPatch` do not
check that a `Read` came first.

## 8. Tool results

Every tool result is data plus one rendering. `ToolResult::report` (`ToolReport`)
holds the status (`success`, `failure`, `timed_out`, `cancelled`, `running`), the
exit code when a process exited normally (never invented: none for timeouts,
signals, launch failures and file tools), the termination cause, the monotonic
duration, the streams as captured (`Streams { stdout, stderr }`, or `Combined`
announced as such), a suggestion tied to an identified error, notes and
structured data. The agent renders:

```text
✓ [Bash] Succès (0.42s) — code 0
--- stdout ---
…
```

```text
✗ [Bash] Échec (1.15s) — code 127
--- stderr ---
bash: foobar: command not found
--- suggestion ---
Vérifiez que l'outil est installé ou disponible dans le PATH.
```

```text
⏱ [Bash] Interrompu après timeout de 120s (121.40s) — aucun code de sortie (2 processus arrêtés)
[Interrompu après timeout de 120s — sortie partielle ci-dessous]
--- stdout ---
…
```

A successful command without output reads `(Commande exécutée avec succès sans
sortie)`; a yielded or background command reads `… [Bash] En cours`, with its
`job_id`, pid and cursors. The header describes the call: it is not
output, and compression never sees it. Notes (`--- remarques ---`: environment
changes not available, raw output paths, processes stopped) and the suggestion
come after the output. Tools that do not provide a report get the same header
from their error flag, without a code.

## 9. Platforms verified

| Platform | What ran |
| --- | --- |
| macOS (arm64), bash 3.2.57 | The shell tests (`crates/bricks-tools/tests/shell_session.rs`), the agent tests (`crates/bricks-agent/tests/shell_tools_e2e.rs`), the file-tool tests. |
| Linux | Not executed. The Unix code path is the same as macOS (bash, `ps`, `setsid`, FIFOs), but nothing was run on Linux. |
| Windows / PowerShell | Not executed, not compiled: no Windows machine, and cross-compilation fails on C dependencies (`ring`). `pwsh` is not installed on the development machine; `crates/bricks-tools/tests/powershell_session.rs` is `#[ignore]` and must be run with `--ignored` where PowerShell exists. Job Objects are not implemented (trees are terminated with `taskkill /T /F`). The Bash tool is unavailable on Windows. |
