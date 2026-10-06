# Shell, file tools and tool results

Contracts of the `Bash`, `PowerShell`, background-task, `Edit`/`MultiEdit`,
`ApplyPatch` and `Read` tools, and of the result format every tool shares.

## 1. Persistent shell

**One live shell per agent session.** An agent's tools share one session id
(its `session_id` when set, else one generated per agent), hence one bash,
one PowerShell and one set of background tasks. Another agent — a sub-agent
included — has its own. Foreground commands in a session are serialised.

**What persists**, for as long as the interpreter lives: the current
directory, variables (exported or not), `PATH`, activated environments
(`source .venv/bin/activate`; nvm/conda when installed and initialised by the
command), aliases, functions, shell options and traps. The process
environment of Bricks itself is never modified.

**What does not**: nothing is reconstructed after the interpreter ends. An
`exit`, an `exec`, a crash, `kill -9 $$`, an enabled `set -e` followed by a
failing command, or a timeout that required destroying the shell end the
session; the result says so, and the next command runs in a fresh shell with
a "session reset" note. Changes made inside a sub-shell `( … )` stay there.

### How bash is driven (Unix)

* One `bash --noprofile --norc` (no user profile is sourced; `BASH_ENV`, `ENV`
  and `PROMPT_COMMAND` are removed), started in its own session (`setsid`),
  without a controlling terminal: a program that would prompt on `/dev/tty`
  (sudo, ssh) fails instead of hanging.
* A small driver runs **at the top level** of that shell (not inside a
  function), so `declare`, `local`-free variables, `return` and aliases behave
  as in an interactive shell. `expand_aliases` is on. Bash ≥ 3.2.
* **Control channel**: requests on fd 198, replies on fd 199 — NUL-separated
  fields with a request id: `READY`, `START id`, `END id status cwd`. Status
  and working directory come only from there; nothing is parsed out of the
  command's output, so output that imitates a marker or the protocol is just
  output.
* **The command's text** is written to a file and sourced (`.`) in the shell's
  scope: quotes, newlines, here-documents and unicode are preserved exactly.
  `return N` ends the command with status N.
* **Streams**: each command gets two FIFOs (stdout, stderr), read from the
  start, concurrently, chunk by chunk — large outputs cannot fill a pipe and
  block. stdin is `/dev/null` unless `input` is given. fds 198/199 are closed
  for the command: `read`, `cat` or a child process can never consume the
  next request.
* Names starting with `__bricks_` are reserved.

Interactions to know: a user `trap … EXIT` runs when the shell ends; `set -e`
persists like in a terminal (a later failing command ends the session,
reported); `exec cmd` replaces the shell (session ends); `exec >file` inside a
command is undone when the command ends.

### Working directories

| Tool | Relative paths resolved against |
| --- | --- |
| `Bash` | the shell's own current directory (changed by `cd`, reported in each result's data as `cwd`) |
| `PowerShell` | the PowerShell session's location (`Set-Location`) |
| `Read`, `Edit`, `MultiEdit`, `Write`, `ApplyPatch`, `Glob`, `Grep` | the agent's working directory — a `cd` in a shell never moves them |
| read-before-edit guard | the agent's working directory |

A `cd` in the shell does not change the agent's working directory nor the
permission root of the workspace.

### Permissions

Permission checks run before execution on the command text, as before. In
addition, when the command invokes an alias or function defined in the
session, its definition is appended to the permission request's
`description`, so session state cannot hide what will actually run (a policy
that matches `rm -rf` sees `alias tidy='rm -rf …'`).

## 2. Execution, timeouts, supervision

| Option (Bash input) | Default | Meaning |
| --- | --- | --- |
| `timeout` | 120 000 ms (max 600 000) | Foreground timeout. |
| `input` | none (empty stdin) | Text given on stdin. Prompts are never answered automatically. |
| `background` | `false` | Start a supervised task instead (§3). |
| `ready_pattern` / `ready_timeout` | — / 10 s | Background only: readiness regex on output lines, and how long to wait for it. |

`ShellConfig` (put in `ToolContext::extensions`, e.g. via
`AgentBuilder::extensions`) sets the defaults: `default_timeout`,
`max_timeout`, `term_grace` (2 s), `drain` (500 ms), `progress_every` (10 s),
capture limits, task log limits, `base_dir`, `bash_path`, `pwsh_path`.

**Progress.** While a command runs, a progress message is emitted every
`progress_every` (`AgentEvent::ToolProgress`): elapsed time, bytes so far,
and — when nothing was printed for a while — that the command may be
computing or waiting for input it will not get. This is a hint, not a
diagnosis: silence does not mean a prompt.

**Timeout.** An event is emitted; the command's process tree (every live
descendant of the shell) gets `SIGTERM`, then `SIGKILL` after `term_grace`;
termination is verified (a process is stopped only when gone or a zombie).
If the shell then reports the end of the command, it keeps its state. If it
does not (a builtin loop such as `while :; do :; done`, a hung shell), the
shell is destroyed and the next command starts a new one, which is reported.
The result has status `timed_out`, no exit code, the partial output and the
number of processes stopped (and forced). Background tasks are not touched.

**Leftovers.** When a foreground command ends, any process it left running
(`cmd &`, a daemon that kept the outputs) is stopped and listed in the
result, with a pointer to `background: true`. Unknown descendants never keep
writing into the streams of a finished command.

**Cancellation.** Cancelling the agent's turn drops the running tool call;
the shell then interrupts the command in the background (tree first, shell
if needed) before taking the next request.

**Output.** Bytes are cleaned incrementally (ANSI CSI/OSC/DCS/APC sequences
and their 8-bit forms, carriage-return progress, backspace, control
characters), across chunk boundaries — including escapes and UTF-8
characters split between reads. Raw bytes are kept in memory up to 4 MiB per
stream, then the complete raw stream is spilled to a file whose path is in
the result; the cleaned view keeps 1 MiB of head and 1 MiB of tail and states
the gap. Output compression (`docs/compression.md`) runs afterwards, on the
cleaned text only.

**Closing.** `Agent::close().await` stops the session's tasks and shells
(graceful, bounded, awaited); dropping the `Agent` does the same
synchronously with forced signals.

### Termination guarantees by OS

| Situation | Unix (macOS, Linux) | Windows |
| --- | --- | --- |
| Normal close / drop | Shell and tasks: whole trees terminated and reaped. | `taskkill /T /F` on each tree. |
| Timeout of a foreground command | Descendants of the shell, found by parent links (`ps`), TERM then KILL, verified. | Not stopped individually: the PowerShell session is destroyed with its tree. |
| Process that double-forks and is re-parented to init (a deliberately detached daemon) | Not found — no longer a descendant. | Not found. |
| Bricks killed with SIGKILL / crash | Nothing runs to clean up: shells and tasks survive in their own sessions. | Same. |

Process groups alone do not contain every descendant (`setsid` escapes a
group); Bricks follows parent links instead, with the limits above. No
administrator privilege is required. Job Objects are not used yet on
Windows (see §8).

## 3. Background tasks

`Bash` with `background: true` returns as soon as the task is started: a
stable `task_id` (`bg-1`, `bg-2`, …), its pid and state. A task is a separate
bash started from a **snapshot** of the persistent shell taken at launch:
working directory, shell options, exported variables, functions and aliases.
Non-exported variables are not inherited. What the task changes never reaches
the persistent shell. It runs in its own session and process group, with its
own pipes: it never blocks the next commands and its output never mixes
with theirs.

* **States**: `running`, `completed` (exit 0), `failed` (non-zero exit, or a
  signal Bricks did not send), `stopped`. The exit code exists only after a
  normal exit.
* **Ready ≠ running**: with `ready_pattern`, readiness is reported when a
  line matches (e.g. `port \d+`); without it, the result says readiness was
  not checked.
* `BashTaskStatus` (one or all tasks), `BashTaskOutput` (`stream`, `offset`
  in lines, `limit` ≤ 2000; returns the next offset, and says when earlier
  lines are no longer in memory), `BashTaskStop` (graceful then forced;
  idempotent).
* **Bounded capture**: 5 000 lines per stream in memory; raw logs up to
  50 MiB per stream in the session directory.
* Tasks are visible and stoppable only from their own session. They are
  stopped when the session closes; a launch that fails leaves nothing behind.

Prefer `background: true` to `cmd &` in a foreground command (whose leftovers
are stopped when the command ends).

## 4. PowerShell

One `pwsh` (or `powershell` on Windows) per session, started with
`-NoProfile -NonInteractive`, its stdin empty. Each request is dot-sourced at
the driver's level, so `$env:…`, `Set-Location`, variables, functions and
aliases persist. Control and output travel on an authenticated loopback TCP
connection (JSON lines), never through the process's stdin/stdout.

The result separates PowerShell's success (`$?`), cmdlet errors (counted),
and the exit code of a native program — reported only when one ran in that
request (`$LASTEXITCODE` is reset before each request, so an old code is
never recycled). A timeout, `exit`, a crash or a lost connection ends the
session with its tree; the next command starts a new one (reported). Output
objects are formatted one by one with `Out-String`.

## 5. Edit and MultiEdit

Three stages, the first unique match wins:

1. **Exact.** Several identical occurrences are refused unless
   `replace_all` (unchanged policy).
2. **Normalised**, line by line, with explicit rules only: LF/CRLF, trailing
   whitespace, one constant indentation shift for the whole block (tabs or
   spaces, tab = 4, 8 or 2 columns) — relative indentation must match, which
   protects Python. Lines inside multi-line strings, template literals, Rust
   raw strings and here-documents must match exactly.
3. **Relocation** (bounded: files ≤ 100 000 lines, blocks ≤ 400 lines): the
   same lines allowing only blank lines and runs of spaces *outside quoted
   text* to differ, with the same constant indentation shift. It finds a block
   whose layout drifted; it never accepts different code, however similar.

The replacement is re-indented by the same shift in the block's style, and
follows the file's line endings; bytes outside the block are untouched. On
refusal, nothing is written and the message says why — absent, ambiguous, or
"the code itself differs" — with the closest regions (line ranges, an
excerpt, similarity). The file is re-read just before writing: if it changed
since the match was resolved, nothing is written. Writes are atomic
(temporary file + rename, permissions kept). A skeleton view still does not
count as having read the file.

## 6. ApplyPatch

Unified diffs (`diff -u`, `git diff`): several files and hunks, creation and
deletion through `/dev/null`, `a/`/`b/` prefixes, timestamps after a tab,
C-quoted paths and plain paths with spaces, CRLF files, `\ No newline at end
of file`. Hunk line counts are checked; context and removed lines must match
the file exactly (at the stated line, or the nearest place after the previous
hunk). Refused explicitly: binary patches, renames, copies, mode changes, a
file appearing twice. Paths must be relative, stay inside the working
directory and go through no symlink. Everything is validated before anything
is written; files are then written atomically one by one, and if a write
fails the files already written are restored (and any that could not be is
named). This is not a multi-file transaction.

## 7. Read

`  12 | text` lines, numbered from 1 and aligned. Pages of `limit` lines
(default 2000) from `offset`; the window is stated against the exact total,
counted by streaming through the file (memory bounded by the page), with the
next offset. A file that changes during the read is flagged. Binary files are
reported with their size and probable type, never shown; UTF-16 and non-UTF-8
text (Latin-1…) are reported as unsupported encodings. Accepted: UTF-8, with
or without a BOM (the BOM is not shown); CRLF is reported in the data. Lines
longer than 4000 characters are cut with a marker. Relative paths are
resolved against the agent's working directory.

## 8. Tool results

Every tool result is data plus one rendering. `ToolResult::report`
(`ToolReport`) holds the status (`success`, `failure`, `timed_out`,
`cancelled`, `running`), the exit code when a process exited normally (never
invented: none for timeouts, signals, launch failures and file tools), the
termination cause, the monotonic duration, the streams as captured
(`Streams { stdout, stderr }`, or `Combined` announced as such), a suggestion
tied to an identified error, notes and structured data. The agent renders:

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

A successful command without output reads `(Commande exécutée avec succès
sans sortie)`; a running task `… [Bash] En cours`. The header describes the
call — it is not output, and compression never sees it. Notes (`--- remarques
---`: session reset, raw output paths, processes stopped) and the suggestion
come after the output. Tools that do not provide a report get the same header
from their error flag, without a code.

## 9. Platforms verified

| Platform | What ran |
| --- | --- |
| macOS (arm64), bash 3.2.57 | Everything in this document: the shell tests (`crates/cersei-tools/tests/shell_session.rs`), the agent tests (`crates/cersei-agent/tests/shell_tools_e2e.rs`), the file-tool tests. |
| Linux | Not executed. The Unix code path is the same as macOS (bash, `ps`, `setsid`, FIFOs), but nothing was run on Linux. |
| Windows / PowerShell | Not executed, not compiled: no Windows machine, and cross-compilation fails on C dependencies (`ring`). `pwsh` is not installed on the development machine; `crates/cersei-tools/tests/powershell_session.rs` is `#[ignore]` and must be run with `--ignored` where PowerShell exists. Job Objects are not implemented (trees are terminated with `taskkill /T /F`). The Bash tool is unavailable on Windows. |
