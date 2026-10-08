# The `bricks` command

`bricks` is a thin layer over the engine (`cersei-agent`): it parses
arguments, loads the configuration, opens a session and presents its
events. The engine runs everything else — the agentic loop, providers,
tools, approvals, compaction, long-term memory recall and maintenance.

```text
bricks-cli (binary `bricks`)
  ├── arguments, configuration (existing loaders)
  ├── headless mode: human output or versioned JSONL
  └── bricks-tui: the interactive interface
            │  commands ↓   ↑ events      (cersei_agent::control)
       engine: Controller → Agent
            │
  providers · context · tools · approvals · memory · sessions
```

The engine never depends on `bricks-cli`, ratatui or crossterm. Both
frontends use the same contract (`cersei_agent::control`): typed
`Command`s in, versioned `Envelope`s out.

## Installation

```bash
cargo install --path crates/bricks-cli
```

This builds the `bricks` binary. The long-term memory uses USearch, a C++
library: a C++17 toolchain is needed to build. During development, run it
with `cargo run -p bricks-cli -- <arguments>`.

## Configuration

Two files are read, both through the existing loaders. Nothing is built in.

* **Models**: `~/.bricks/providers.toml`, or `--providers <file>`, or
  `$BRICKS_HOME/providers.toml` when `BRICKS_HOME` is set (see
  `docs/providers.md`). A model is named `provider_id/model_id`; its
  reasoning profiles, capabilities and prices come from this file.
* **Project**: `bricks.toml` in the project folder (the current folder, or
  `--workspace <dir>`; see *Projects* below). Sections: `[agent]`, `[permissions]`, `[memory]`,
  `[context]`, `[compression]`, `[web]` (see `docs/bricks.example.toml`).
  An invalid section falls back to its defaults with a diagnostic, shown
  as a warning when the session opens.

```toml
[agent]
model = "my-provider/my-model"   # default model (otherwise --model is required)
reasoning = "deep"               # one of that model's reasoning profiles
max_turns = 50

[permissions]                    # the approval policy (below)
write = "ask"
execute = "ask"

[permissions.tools]
WebFetch = "allow"
```

Sessions are stored in `~/.bricks/sessions` (`$BRICKS_HOME/sessions`).

No model is ever chosen silently: without `--model` or `[agent] model`,
`bricks` stops and lists the configured models.

## Projects (`--workspace`)

```bash
bricks                                          # the current folder
bricks --workspace /Users/me/Documents/atlas
bricks --workspace ~/Documents/atlas
bricks --workspace "~/Documents/My project"     # quoted: Bricks expands `~` itself
bricks --workspace ./my-project
bricks --cd ./my-project                        # alias of --workspace
bricks --workspace ./my-project sessions        # global: before or after the command
bricks sessions --workspace ./my-project
bricks --workspace ./my-project run "Describe this project."
```

* **One option.** `--workspace` (alias `--cd`) is global. Given twice —
  under either name — it is refused (exit code 2), with no hidden
  priority.
* **Resolution**, without any shell: a leading `~` or `~/` is the home
  folder (also when quoted, with spaces kept); `~user` and variables are
  not expanded; a relative path starts from the folder Bricks was started
  in. The result is made canonical (symbolic links resolved) and must be a
  readable folder. A missing folder or a file is refused before anything
  is created, stored or sent. Bricks never creates the folder.
* **Where it applies.** The folder of a new session: its `bricks.toml`,
  rules, system prompt, long-term memory, sub-agent profiles, the tools'
  relative paths, CodeScout and the completion list. Bricks itself never
  changes its process's current folder: tools resolve paths against their
  agent's folder (a sub-agent's worktree has its own).
* **Other paths** on the command line (`--providers`, `--file`,
  `--image`): `--providers` starts from the folder Bricks was started in;
  attachments are resolved against the session's folder, as before.
* **Shown.** The status bar ends with the project (`⌂ ~/Documents/atlas`,
  shortened with `…/` and a short mark of the full path when long);
  `/session` and the session picker show the full path.
* **Not a sandbox.** A workspace chooses settings and CodeScout's scope;
  the other tools follow the approval policy, as everywhere. A workspace
  is a folder; a worktree (`docs/agents.md`) is another folder, hence
  another workspace; neither is an isolation of the system.

## Commands

| command | what it does |
|---|---|
| `bricks`, `bricks tui` | open the interactive interface (new session) |
| `bricks run "…"` | run one prompt without the interface |
| `bricks run --model <p/m> --reasoning <profile> "…"` | with another model / profile |
| `bricks run --json --non-interactive "…"` | JSONL on stdout, no human interaction |
| `bricks run --session <id> "…"` | continue a stored session headless |
| `bricks sessions [--json]` | the project's stored sessions, most recent first |
| `bricks sessions --all [--json]` | every stored session, whatever the project |
| `bricks resume` | open the interface on the session picker (this project; Tab: all) |
| `bricks resume <id>` | open the interface on that session (any project) |

Global options: `--workspace <dir>` (alias `--cd`), `--providers <file>`,
`--model <p/m>`, `--reasoning <profile>`, `--hyperlinks auto|always|never`.

## Headless mode (`bricks run`)

It works without a terminal: no terminal mode is changed.

**Prompt and stdin.** The rules are fixed:

| invocation | prompt |
|---|---|
| `bricks run "text"` | `text`; stdin is not read, even when piped |
| `bricks run -` | stdin |
| `bricks run` (stdin not a terminal) | stdin |
| `bricks run` (stdin is a terminal) | error, exit code 2 (never waits) |
| `bricks run "text" --stdin` | `text`, then stdin as a second block (`<stdin>…</stdin>`) |

`--file <path>` and `--image <path>` attach files and images. Their
content is captured when the run starts.

**Output.** Without `--json`, the answer is streamed to stdout. Tool
calls, notices and errors go to stderr. With `--json`, stdout carries
only JSONL envelopes (schema below). Diagnostics and logs go to stderr.

**Approvals.** They are asked on the controlling terminal (`/dev/tty`)
when there is one, with the diff of a file change. With
`--non-interactive`, or without a terminal, nobody is waited for. A step
the policy says to ask about then stops the run, with `run_finished`
`failure = "approval_required"` and exit code 3. `--non-interactive`
grants nothing: steps the policy allows still run, the others do not.

**Ctrl+C** cancels the run (or the memory maintenance after it). A second
Ctrl+C exits at once. The session is stored. Tool calls interrupted by the
cancellation get a result saying so, so the session can be continued.
Effects they already had remain: nothing is "undone".

**Memory maintenance.** After the answer, the long-term memory (when
enabled) processes the exchange. `bricks run` waits for it unless
`--no-wait-memory` is given. Its work is durable and resumes at the next
run.

### Exit codes

| code | meaning |
|---|---|
| 0 | the run succeeded |
| 1 | the run failed (provider, engine, tool infrastructure), or stdout closed |
| 2 | invalid arguments or configuration, unknown model or session, refused attachment: nothing ran |
| 3 | a step needed an approval and nobody could give it |
| 4 | the answer was delivered, but the long-term memory maintenance failed |
| 5 | the run stopped at a limit before a final answer (`outcome = incomplete`); the partial answer was printed |
| 6 | the answer was delivered, but background sub-agents did not complete: still running after `[agents] background_drain_ms` (then cancelled), failed or cancelled; in text mode one line per agent is printed on stderr, with `--json` the `agent_finished` and `run_usage` events say it |
| 130 | cancelled (Ctrl+C) |

### When a run stops

A run ends when the model gives a final answer with no tool call left: tools
being available is never an obligation to use them, and Bricks adds no
"use more tools" or "read more files" relaunch. Every other way a run can
continue has a visible cause and a bound:

| continuation | cause shown | bound |
|---|---|---|
| tool calls | `tool_started` / `tool_finished` | `max_turns` (`[agent]`, default 50): `N` allows at most `N` generation turns; reaching it ends the run `incomplete` (`max_turns`) |
| answer cut by the output-token limit | `notice` "continuing (n/3)"; calls in the cut answer are answered as not run | 3 continuations, each one a turn; then `incomplete` (`output_truncated`) |
| the same calls returning the same results | `notice` "No progress…" once, after 3 repeated rounds | 5 repeated rounds: `incomplete` (`no_progress`). Different arguments or results (another file, a test run again after an edit) are progress |
| transport errors (429, 5xx, network) | `notice` "Retrying in …" | 5 retries of one request with backoff; not turns; then `failed` |
| request refused as too long | `compaction` | the context policy's `max_overflow_recoveries` per turn |

When the run stops at a limit, the history, partial answer and tool results are
kept, every tool call has a result, and no tool is started afterwards. The
memory maintenance that follows `run_finished` never restarts the task.

Sub-agents (the native `Agent` tool, registered in the session agent unless
`[agents] enabled = false`; see `docs/agents.md`) refuse an invalid request
before anything is built, get their parent's permissions and at most its
tools (never a delegation tool), are cancelled with the parent's run (which
waits for their cleanup), and report `completed`, `incomplete`, `cancelled`
or `failed` with their partial answer. In headless mode, delegating needs
`Agent` / `Agents` allowed in `[permissions]` (`execute` tools); allowing
them allows none of the children's own tools. Background sub-agents are
drained after the answer (bounded by `[agents] background_drain_ms`), then
cancelled: exit code 6 if any did not complete.

## The JSONL schema (version 4)

Every line is one envelope:

```json
{"schema":4,"session_id":"20261006-141502-a1b2c3","run_id":"run_5f…","seq":7,"at":1791300902123,"type":"tool_started","tool_call_id":"call_1","name":"Glob","input":{"pattern":"*.md"}}
```

| field | |
|---|---|
| `schema` | `4`; incremented on any change a consumer could notice (2: `run_finished` gained `incomplete` and `termination`; 3: the `search` command and its `search_results` event; 4: sub-agents — `agent_*` events, `agent_id` in `approval_requested`, `list_agent_profiles` and `reload_agent_profiles`). Additions a consumer can ignore — new event types, new optional fields, new commands, new states — keep the version (Sprint 10.5's runtime is such an addition); a removed or renamed field or event, or a changed meaning, increments it. Consumers ignore unknown types and fields |
| `session_id` | the session |
| `run_id` | the run (absent for session-level events) |
| `seq` | 1, 2, 3, … contiguous: a gap never happens silently |
| `at` | milliseconds since the Unix epoch |
| `type` | the event (below); its fields follow, flat |

Events (`type`):

| type | fields | |
|---|---|---|
| `session_opened` | `working_dir, model, reasoning?, resumed, message_count, warnings` | first event |
| `run_started` | `prompt, attachments[{kind,path,bytes,sha256?,note?}], model, reasoning?` | |
| `text_delta` | `text` | the answer, streamed |
| `thinking_delta` | `text` | only reasoning the provider exposed; never reconstructed |
| `tool_started` | `tool_call_id, name, input` | concurrent calls have distinct ids |
| `tool_progress` | `tool_call_id?, name, message` | long calls (shell) |
| `tool_finished` | `tool_call_id, name, is_error, duration_ms, output` | |
| `approval_requested` | `approval{approval_id, tool_call_id, tool, level, description, input, preview?, agent_id?}` | `preview.files[{path, kind, before_sha256, diff, added, removed}]` |
| `approval_resolved` | `approval_id, tool_call_id, decision, by` | `decision`: `allow`, `allow_for_session`, `deny`; `by`: `user`, `session`, `non_interactive`, `cancelled` |
| `edit_applied` | `tool_call_id, tool, files[{path, kind, added, removed}]` | an approved, previewed change was written |
| `memory_recalled` | `items, tokens, omitted, budget` | what went into the system prompt |
| `context` | `status{context_used{tokens, provenance, …}, context_window, input_limit, totals, …}` | `provenance`: `measured`, `counted`, `mixed`, `estimated` |
| `usage` | `turn{…}, total{…}` | observed token usage; `cost_usd` absent = unknown, not zero |
| `compaction` | `reason, outcome, compacted` | |
| `model_changed` | `model, reasoning?, applies` | `next_turn` during a run, `next_run` otherwise |
| `context_cleared` | `messages_removed` | |
| `session_saved` | | |
| `notice` | `message` | retries, configuration diagnostics, continuations after a cut answer, no-progress warnings |
| `run_finished` | `outcome, failure?, error?, termination?, text, turns, approvals_unsatisfied?` | **exactly one per run**; `outcome`: `succeeded`, `incomplete`, `failed`, `cancelled`; `failure`: `approval_required`, `error`; `termination.kind`: `completed`, `max_turns` (`limit`), `output_truncated` (`continuations`), `no_progress` (`repeats`), `content_filtered`, `empty_response`; `turns`: generation turns that got a response |
| `memory_maintenance_started` | | after `run_finished` |
| `memory_maintenance_finished` | `outcome, report?, error?` | `outcome`: `completed`, `cancelled`, `failed` |
| `search_results` | `query, status, hits[{path, line, column, text}], omitted, notes?, elapsed_ms` | answer to `search`; `line`/`column` 1-based, column in characters; `status`: `complete`, `partial` (a limit was reached: absence proves nothing), `cancelled`, `error` |
| `agent_spawned` | `agent{agent_id, parent_id?, root_run_id, tool_call_id?, batch_index?, background, depth, profile, profile_source, profile_revision, model{requested, applied, reason?}, reasoning{…}, max_turns, workspace, isolation, branch?, task, created_at}` | a sub-agent was created (see `docs/agents.md`); `tool_call_id` is the parent's call |
| `agent_state` | `agent_id, state, reason?` | `queued`, `waiting_admission`, `starting`, `running`, `cancelling`, then one of `completed`, `incomplete`, `failed`, `cancelled`, `interrupted` (found unfinished when the session reopened) |
| `agent_tool_started` / `agent_tool_finished` | `agent_id, tool_call_id, name, input` / `…, is_error, duration_ms` | the sub-agent's tool calls (its text is never streamed) |
| `agent_finished` | `result{agent_id, profile, status, termination?, error?, summary, files_changed, commands, warnings, turns, usage, duration_ms, model, reasoning?, workspace, transcript?, changeset?, branch?, skills?}` | the compact result; its usage is not in the run's `usage` events (see `run_usage`) |
| `agent_profiles` | `profiles[{name, description, scope, source, valid, error?, shadows}], total, page, diagnostics?` | answer to `list_agent_profiles` / `reload_agent_profiles` |
| `changes_ready` | `changeset{id, agent_id, task, workspace, branch, base_sha, snapshot_id, baseline_tree, final_tree, files[{path, status, from?, added?, removed?, binary}], patch, patch_bytes, state, validations, note?}` | an isolated sub-agent ended with changes; nothing applied |
| `changes_updated` | `changeset_id, state, files, detail?` | `applied`, `conflict` (nothing written), `discarded` |
| `run_usage` | `root_run_id, own, descendants, total, final_total, pending_agents` | after a run, then once more when its last background descendant ends (`final_total: true`); each agent counted once |
| `agent_control_result` | `action, ok, text` | answer to `agent_control` |
| `job_started` / `job_output` / `job_finished` | `job_id, agent_id, root_run_id, command, cwd, pid` / `job_id, stdout_bytes, stderr_bytes` / `job_id, state, code?, signal?, duration_ms, logs` | background commands; `job_output` carries counts, never the output |
| `command_rejected` | `command, reason` | nothing changed |

A short run:

```text
{"schema":4,"session_id":"…","seq":1,"at":…,"type":"session_opened","working_dir":"/p","model":"demo/scripted","resumed":false,"message_count":0,"warnings":[]}
{"schema":4,"session_id":"…","run_id":"run_…","seq":2,"at":…,"type":"run_started","prompt":"Find the README","attachments":[],"model":"demo/scripted"}
{"schema":4,…,"seq":3,"type":"tool_started","tool_call_id":"call_0","name":"Glob","input":{"pattern":"*.md"}}
{"schema":4,…,"seq":4,"type":"tool_finished","tool_call_id":"call_0","name":"Glob","is_error":false,"duration_ms":3,"output":"README.md"}
{"schema":4,…,"seq":9,"type":"text_delta","text":"I listed the Markdown files. …"}
{"schema":4,…,"seq":14,"type":"run_finished","outcome":"succeeded","termination":{"kind":"completed"},"text":"…","turns":3}
```

No API key, authentication header or secret appears in any event. The
views the frontends read do not carry them.

### Commands (for frontends)

```json
{"type":"submit","prompt":{"blocks":[{"kind":"text","text":"…"},{"kind":"file","path":"src/a b.rs"}]}}
{"type":"cancel"}
{"type":"set_model","model":"p/m","reasoning":"deep"}
{"type":"compact"}
{"type":"clear_context"}
{"type":"resume","session_id":"…"}
{"type":"approve","approval_id":"ap_…","decision":"allow_for_session"}
{"type":"search","text":"needle","regex":false}
{"type":"list_agent_profiles","query":"review","page":0}
{"type":"reload_agent_profiles"}
{"type":"agent_control","action":"apply_changes","changeset_id":"cs_…"}
{"type":"agent_control","action":"stop_job","job_id":"job_…"}
```

`agent_control.action`: `list`, `status`, `result`, `cancel` (`agent_id`),
`inspect_changes`, `apply_changes`, `discard_changes` (`changeset_id`),
`jobs`, `stop_job` (`job_id`). Frontends see every instance and job of the
session.

Rules during a run:

* `cancel` is always accepted and reaches the run at once.
* `set_model` is accepted and applies from the next turn (`model_changed`
  says so). Profiles are the configuration's; there is no fixed list.
* `submit`, `resume`, `compact` and `clear_context` are refused
  (`command_rejected`). A second prompt never starts silently in
  parallel.
* `list_agent_profiles`, `reload_agent_profiles` and `agent_control` are
  accepted at any time; a reload does not change a running sub-agent.
  `cancel` while idle cancels pending background sub-agents.
* `search` is read-only and accepted at any time; it uses the workspace's
  shared code engine (the one the agents' `CodeScout` uses) and never
  starts a language server (see `docs/semantic.md`).

Presentation gestures (opening a window, expanding a block) are not
commands: they stay in the frontend.

### Delivery

Events reach each consumer through a bounded queue:

* text and reasoning deltas are merged when the consumer is slow. The
  text stays whole; only the number of events drops.
* any other event waits for room, so the run slows to the consumer's
  pace. Nothing is dropped.
* a cancellation is never held back by a slow consumer. Once the run is
  cancelled, its last events are queued beyond the bound.
* when the consumer goes away (closed stdout, closed interface), the run
  is cancelled (headless) or the session closes.

The durable session store is the authority on the conversation. The
interface keeps no conversation store of its own. On resume it reads the
stored history.

## Approvals

The policy belongs to the engine and is the same for the interface and
the headless mode. It is configured in `[permissions]`:

* by tool name first (`[permissions.tools]`), then by permission level
  (`read_only`, `write`, `execute`, `dangerous`, `none`), then `default`;
* actions: `allow`, `ask`, `deny`;
* defaults: reading is allowed, writing and executing are asked;
  `forbidden` tools are always refused.

When a call must be asked about:

* **File changes** (`Write`, `Edit`, `MultiEdit`, `ApplyPatch`). The tool
  computes the change without writing: files, kind, diff, and the SHA-256
  of each file at that time. Nothing is written before the decision.
* **A file changed meanwhile.** If a file changed while the decision was
  pending, the approval is not applied to the new state. The change is
  recomputed and asked again; after three changes in a row it is refused.
  The tiny window between that check and the write is not locked.
* **Rejection.** A rejected change writes nothing and leaves your files as
  they are.
* **Shell commands and MCP calls** have no file preview: their effects
  cannot be shown as a diff, and none is pretended. The approval shows the
  command and its context.
* **Allow for the session** applies to later calls of the same tool in the
  session.

## The interactive interface

* **Layout.** The interface is conversational: the transcript, a composer
  at the bottom and a one-line status bar. It uses an inline viewport. The
  live part (the run in progress, the composer, the status) stays at the
  bottom; finished parts are written once into the terminal's own
  scrollback, so the transcript stays in your terminal after you quit.
* **Windows.** Inspectors, pickers and the diff open on the alternate
  screen, and closing them restores the transcript untouched.
* **Long outputs** are never redrawn on every frame. In the transcript, a
  tool call shows one line (or its first error line). Runs of more than six
  calls are grouped. The details of the last calls (outputs up to 256 KB
  each; originals are stored with the session) open with Ctrl+O.
* **Reasoning** appears as a collapsed block only when the provider exposed
  some. Ctrl+T shows it live; Ctrl+O shows it after the fact.
* **Status bar.** It shows the model, the profile, the context used and the
  prompt budget, with the provenance (`est.`, `~`, `counted`, or nothing for
  measured), then the project folder. The cost is "unknown (no price)" when
  the model has no price.
  The full context and cost inspectors separate the current context from
  the cumulative consumption.

### Keys

| key | |
|---|---|
| Enter | send |
| Shift+Enter, Alt+Enter, Ctrl+J | newline (Shift+Enter needs a terminal that reports it, e.g. kitty, WezTerm, foot, recent iTerm2) |
| `\` then Enter | newline, on any terminal |
| paste | inserted as text (bracketed paste): never sent |
| `@` | file and folder list (fuzzy, `.gitignore` respected); Tab/Enter attach, Ctrl+R refresh, Esc close; `@"name with spaces` |
| `/` | command list |
| ↑ / ↓ | prompt history (on the first / last line); otherwise move the cursor |
| Ctrl+←/→, Alt+←/→ | by word; Shift selects |
| Ctrl+W, Ctrl+U | delete a word, clear the input |
| Backspace on an empty input | remove the last attachment |
| Ctrl+C | cancel the run or the memory maintenance; idle: clear the input, twice in a row: quit |
| Ctrl+D | quit (on an empty input) |
| y / a / n / d | with a pending approval: allow, allow for the session, reject, open the diff. Tab moves between the approval and the input |
| Ctrl+T, Ctrl+O | live reasoning; details of the last tools and reasoning |

### Commands

`/model [p/m [profile]]` · `/search <text>` (`re:<regex>`) ·
`/agents [words | reload | running | status <id> | result <id> | cancel <id>]` ·
`/changes <id> [inspect | apply | discard]` · `/jobs [stop <id>]` · `/memory` · `/context` · `/cost` · `/session` ·
`/resume [id]` · `/compact` · `/clear` · `/diff` · `/tools` · `/mcp` ·
`/config` · `/file <path>` · `/folder <path>` · `/image <path>` · `/help` ·
`/quit`.

* **Registry.** The registry is declarative (name, aliases, arguments,
  description, action). Engine actions are contract commands; the others
  only open windows.
* **`/diff`** shows three different things apart: the change waiting for a
  decision, the changes applied by the agent in this session, and the
  working tree's own `git diff`, which includes your changes.
* **`/config`** never shows keys or authentication headers.

### Web links

Links in answers keep their text and their address, written next to it
(`la doc <https://example.com/doc>`): what a click opens is always
visible. `--hyperlinks` decides whether they are clickable (OSC 8):

| mode | |
|---|---|
| `auto` (default) | clickable when the terminal is recognised: iTerm2, WezTerm, Ghostty, VS Code (`TERM_PROGRAM`), kitty, Windows Terminal, VTE ≥ 0.50, Konsole. Off inside tmux or screen, and in any other terminal (`TERM=xterm-256color` alone proves nothing; Apple's Terminal is not recognised) |
| `always` | clickable in any terminal (an emulator without OSC 8 support shows the text only) |
| `never` | never |

* The terminal opens a link with its own gesture (⌘-click in iTerm2);
  Bricks never opens a browser and never fetches an address to check it.
* Only `http`/`https` addresses with a host are activated, in their
  percent-encoded, printable form: nothing in an address can end the
  sequence or start another one. Local paths, `file:`, `javascript:` and
  invalid addresses stay plain text; control characters in an address are
  shown as `�`.
* A wrapped link opens the same address from every piece; links survive
  resizing, scrolling into the scrollback and resuming (a restored answer
  is rendered like a new one); text drawn over a former link is plain.
* Nothing of this is stored: the conversation and the JSONL events keep
  the Markdown. `bricks run` (text or `--json`) never writes OSC 8; its
  text output prints the answer as written, without a Markdown renderer.

### Attachments

* **What.** Files, folders and images come from `@` or `/file`,
  `/folder`, `/image`.
* **Capture.** A file's content is captured when the prompt is sent: its
  path, size and SHA-256 go into the message, and the session keeps
  exactly what was sent. A later edit of the file does not change the
  history.
* **Folders** become a bounded listing (300 entries, depth 4,
  `.gitignore` respected), never their recursive contents.
* **Images** are sent as image blocks. The provider refuses them before
  sending when the model does not declare image input.
* **Paths** are resolved against the working directory when the prompt is
  sent. A missing one refuses the submission, so nothing is sent.

## Sessions and resume

**History by project.** `bricks sessions` lists the sessions of the
project (the current folder, or `--workspace`); `--all` lists every one.
`--json` writes the same selection as one `SessionSummary` object per line
and nothing else (no session: no line, exit code 0). Before Sprint 11,
`sessions --json` listed every session: scripts that want that keep it
with `sessions --all --json`. A project's sessions are those whose
recorded folder is the same folder (canonical path: a symbolic alias
matches; `atlas` is not `atlas-old`; a sub-folder or another worktree is
another project). Older sessions without a recorded folder, and sessions
whose folder was deleted, appear only with `--all`, said so. Listing reads
only: no provider, model or key is needed, and nothing is created, moved
or changed. The session store is unchanged (one store, no copy per
project).

**Picker.** `bricks resume`, `/sessions` and `/resume` without an id open
one picker: the sessions of the open session's project (after a resume,
that session's project). **Tab** switches between *this project* and *all
projects* (shown at the bottom); typing filters either list. An empty
project list says so and points to Tab.

**Resuming another project's session** is allowed: `resume <id>`,
`run --session <id>` and the picker take any id. It opens session B in
B's recorded folder with B's project — its `bricks.toml`, rules, system
prompt, long-term memory, profiles, CodeScout settings. It does not turn
the session you were in into a session of B, and `--workspace` does not
override B's folder. The session is read first: the settings of the
folder you started from are not loaded for it (a broken `bricks.toml`
there does not prevent resuming B). Everything B needs is prepared before
anything is switched: when that fails, the open session stays as it was
and the reason is shown. Approvals "for the session" stay with the
session they were given in; the previous session's background jobs stop.

`bricks resume` (picker) or `bricks resume <id>`, `/resume`, and
`bricks run --session <id>` restore:

* the conversation (the active history and the raw history) and the
  compaction snapshots;
* the working directory;
* the model and reasoning profile;
* the long-term memory space, recorded with the session (a different
  current space is reported as a warning).

A working directory that disappeared, or a model no longer in
`providers.toml`, is reported and nothing is resumed: there is no fallback
to the current folder. No replacement is chosen silently; `--model` picks
one explicitly. An older session without recorded settings resumes in the
current project folder with the selected model, and a warning says which.

The persistent shell has its own life cycle. A resumed session starts a
new shell: processes, background tasks and shell variables of the old one
are not restored.

Deleting a session (`Memory::delete`) removes its history, raw history,
snapshots, saved tool outputs and `session.json` (its settings).

## Long-term memory

With `[memory] enabled = true` (see `docs/memory.md`), each run's prompt
recalls stored facts (`memory_recalled`). After the answer, the exchange
is processed as a separate, cancellable phase
(`memory_maintenance_started` / `_finished`). That phase is durable and
resumable: an interrupted pass leaves its work pending, and a successful
extraction is never repeated. A maintenance failure is reported on its
own; it never turns into a "memory success", and the delivered answer is
unaffected.

## A demonstration without any key

`scripts/demo_model.py` is a local scripted model server (Python standard
library). It speaks `chat_completions` on 127.0.0.1:8765, with
`scripts/demo_providers.toml`.

```bash
python3 scripts/demo_model.py &
export BRICKS_HOME="$PWD/.demo-home"
printf '[agent]\nmodel = "demo/scripted"\n' > bricks.toml   # in a scratch directory
bricks --providers scripts/demo_providers.toml run --json --non-interactive "Find the README"
bricks --providers scripts/demo_providers.toml run --non-interactive "please write a file"; echo "exit $?"   # 3: approval needed
bricks --providers scripts/demo_providers.toml sessions
bricks --providers scripts/demo_providers.toml            # the interface
```

## Tested platforms

* **macOS (arm64).** This is the only platform these tests and the PTY
  checks ran on: a Ctrl+C in headless mode, and the interface's modes,
  paste, resize, Ctrl+C and quit. The `/dev/tty` approvals and the PTY
  tests are Unix-only.
* **Web links (Sprint 11).** Checked on the bytes the interface writes in
  a pseudo-terminal (modes `auto`/`always`/`never`, a recognised terminal,
  an unknown one, tmux) and replayed cell by cell by a test emulator. Not
  yet checked by clicking in a real emulator: the only terminal on the
  test machine is Apple's Terminal, which `auto` does not recognise (text
  and address shown); clickability in iTerm2 and the others listed above is
  expected from their documented OSC 8 support, not verified here.
* **Linux.** Not run.
* **Windows.** Not run: the headless mode builds without `/dev/tty`
  approvals (no prompt is offered), and the interface was not exercised.

## Known limits

* The engine keeps its built-in nudges, notices included. After an early
  answer that used tools, it asks once for a deeper look. A first answer
  without tools is retried once with a forced tool call. This is existing
  engine behaviour, not a CLI choice.
* Tool progress events carry the tool's name, not its call id (the
  engine's progress hook does not know the call).
* A sub-agent's events carry its `agent_id`, parent, root run and the
  parent's `tool_call_id` (and `batch_index` within `Agents`).
* No MCP server is configured from `bricks.toml` yet: `/mcp` says so.
* No syntax highlighting, clipboard access or in-terminal image preview.
  Images are attached by path.
* A panic restores the terminal through a panic hook. This is not covered
  by an automated test.
* Two Bricks processes may open the same project or resume the same
  session at once; nothing locks between processes.
* A session's project cannot be changed while it is open (no
  `/workspace`); a workspace is one folder.
