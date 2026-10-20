# The `bricks` command

`bricks` is a thin layer over the engine (`bricks-agent`): it parses
arguments, loads the configuration, runs one prompt or lists the stored
sessions, and presents the engine's events. The engine runs everything
else — the agentic loop, providers, tools, approvals, compaction,
long-term memory recall and maintenance.

```text
bricks-cli (binary `bricks`)
  ├── arguments, configuration (existing loaders)
  └── headless mode: human output or versioned JSONL (run, sessions)
            │  commands ↓   ↑ events      (bricks_agent::control)
       engine: Controller → Agent
            │
  providers · context · tools · approvals · memory · sessions

apps/bricks-desktop: the interactive interface, on the same contract
```

The engine never depends on `bricks-cli` or on any interface. The two
frontends use the same contract (`bricks_agent::control`): typed
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
  `--workspace <dir>`; see *Projects* below). Sections: `[agent]`,
  `[permissions]`, `[agents]`, `[bash]`, `[background]`, `[semantic]`,
  `[memory]`, `[context]`, `[compression]` and `[web]` (see
  `docs/bricks.example.toml`, which lists every key with its default).
  An invalid section falls back to its defaults with a diagnostic, shown
  as a warning when the session opens.

```toml
[agent]
model = "my-provider/my-model"   # default model (otherwise --model is required)
reasoning = "deep"               # one of that model's reasoning profiles

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
bricks sessions                                 # the sessions of the current folder
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
* **Shown.** The desktop application shows the project's folder;
  `/session` shows the full path.
* **Not a sandbox.** A workspace chooses settings and CodeScout's scope;
  the other tools follow the approval policy, as everywhere. A workspace
  is a folder; a worktree (`docs/agents.md`) is another folder, hence
  another workspace; neither is an isolation of the system.

## Tools of a session

The ADK has a larger built-in tool set than a `bricks` session gets. The
difference matters when you read the catalogue or build an agent with the
library. A session of the `bricks` command or of Bricks Desktop has:

* **In every session** (`bricks_tools::coding()`): `Read`, `Write`, `Edit`,
  `ApplyPatch`, `Glob`, `Grep`, `CodeSearch`, `CodeScout`, `NotebookEdit`,
  `Bash`, `BashTaskStatus`, `BashTaskOutput`, `BashTaskStop`, `Job`,
  `PowerShell`, `WebFetch` and `WebSearch`.
* **With skills** (both frontends enable them): `Skill`.
* **When `[agents] enabled` is true, the default**: `Agent`, `Agents`,
  `AgentControl` and `AgentProfiles`.

The rest of `bricks_tools::all()` is not given to a session: planning
(`TodoWrite`, `EnterPlanMode`, `ExitPlanMode`), scheduling (`CronCreate`,
`CronList`, `CronDelete`, `Sleep`, `RemoteTrigger`), orchestration
(`SendMessage`, the `Task*` tools, `EnterWorktree`, `ExitWorktree`),
`AskUserQuestion`, `SyntheticOutput`, `Config` and `ToolSearch`. An agent you
build with the library chooses its own tools.

Every tool, whatever its set, goes through the approval policy of
`[permissions]`: each tool has a permission level (`read_only`, `write`,
`execute`, `dangerous` or `none`), and the policy maps the level, or the tool
name, to `allow`, `ask` or `deny`. The defaults are in the `[permissions]`
section of `docs/bricks.example.toml`. The built-in tools page of the site
(`docs/content/docs/built-in-tools.mdx`) describes each tool.

## Commands

| command | what it does |
|---|---|
| `bricks` | prints the help. The command line has no interactive interface: see [The desktop application](#the-desktop-application) |
| `bricks run "…"` | run one prompt and print the answer |
| `bricks run --model <p/m> --reasoning <profile> "…"` | with another model / profile |
| `bricks run --json --non-interactive "…"` | JSONL on stdout, no human interaction |
| `bricks run --session <id> "…"` | continue a stored session, in its own folder and project |
| `bricks run --skill <name> [--skill …] "…"` | activate skills before the prompt (see `docs/skills.md`) |
| `bricks run --trust-project-skills …` | allow this project's skills for this session only |
| `bricks sessions [--json]` | the project's stored sessions, most recent first |
| `bricks sessions --all [--json]` | every stored session, whatever the project |

Global options: `--workspace <dir>` (alias `--cd`), `--providers <file>`,
`--model <p/m>`, `--reasoning <profile>`.

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

**Skills.** `--skill <name>` (repeatable) activates skills, in order,
before the prompt, which stays a separate message. Every one is checked
before anything is sent; one that is missing, not approved, invalid or
incompatible stops the command with exit code 2 and nothing is sent. The
project's skills (`.agents/skills`, `.claude/skills`) are used only with
`--trust-project-skills`, for this project and this session; without it a
`notice` says they were found and not used, and nothing waits for an
answer. The flag grants no other permission. See `docs/skills.md`.

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
Ctrl+C exits at once. The session is stored. Tool calls already finished
keep their result; calls interrupted by the cancellation get a result
saying so, so the session can be continued. Effects they already had
remain: nothing is "undone".

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
| 6 | the answer was delivered, but background sub-agents did not complete: still running when `[agents] background_drain_ms` runs out (then cancelled), failed or cancelled; in text mode one line per agent is printed on stderr, with `--json` the `agent_finished` and `run_usage` events say it. The default is `background_drain_ms = 0`: background sub-agents still running at the end of the main run are cancelled at once |
| 130 | cancelled (Ctrl+C) |

### When a run stops

A run ends when the model gives a final answer with no tool call left: tools
being available is never an obligation to use them, and Bricks adds no
"use more tools" or "read more files" relaunch. Every other way a run can
continue has a visible cause and a bound:

| continuation | cause shown | bound |
|---|---|---|
| tool calls | `tool_started` / `tool_finished` | no turn limit (removed in 0.4.8): the run goes on until its answer or one of the stops below, or a cancellation (Ctrl+C) |
| answer cut by the output-token limit | `notice` "continuing (n/3)"; calls in the cut answer are answered as not run | 3 continuations, each one a turn; then `incomplete` (`output_truncated`) |
| the same calls returning the same results | `notice` "No progress…" once, after 3 repeated rounds | 5 repeated rounds: `incomplete` (`no_progress`). Different arguments or results (another file, a test run again after an edit) are progress |
| transport errors (429, 5xx, network) | `notice` "Retrying in …" | 5 retries of one request with backoff; not turns; then `failed` |
| request refused as too long | `compaction` | the context policy's `max_overflow_recoveries` per turn |

A former `max_turns` (`[agent]`, `[agents] default_max_turns`,
`max_turns_cap`, a profile's `max_turns`) is ignored and reported as a
diagnostic when the session opens; nothing is rewritten in your files.
A result stored before 0.4.8 may still say `max_turns`: it is read as
such; no run produces it any more.

When the run stops at a limit, the history, partial answer and tool results are
kept, every tool call has a result, and no tool is started afterwards. The
memory maintenance that follows `run_finished` never restarts the task.

Sub-agents (the native tools `Agent`, `Agents`, `AgentControl` and
`AgentProfiles`, registered in the session agent unless `[agents] enabled =
false`; see `docs/agents.md`) refuse an invalid request before anything is
built, get their parent's permissions and at most its tools (never the
sub-agent tools themselves), are cancelled with the parent's run (which
waits for their cleanup), and report `completed`, `incomplete`, `cancelled`
or `failed` with their partial answer. In headless mode, starting sub-agents
needs `Agent` / `Agents` allowed in `[permissions]` (`execute` tools);
allowing them allows none of the children's own tools. Background sub-agents
are drained after the answer, for at most `[agents] background_drain_ms`
(`0` by default, so they are cancelled at once), then cancelled: exit code 6
if any did not complete.

When a provider's `Retry-After` is longer than `[agent] retry_after_max_wait_ms`
(`60000` by default), the run does not wait for it: no retry is sent, and the
run fails at once with an error that names the setting. Shorter waits are
taken, and each one is shown by a `Retrying in …` notice.

## The JSONL schema (version 6)

Every line is one envelope:

```json
{"schema":6,"session_id":"20261006-141502-a1b2c3","run_id":"run_5f…","seq":7,"at":1791300902123,"type":"tool_started","tool_call_id":"call_1","name":"Glob","input":{"pattern":"*.md"}}
```

| field | |
|---|---|
| `schema` | `6`; incremented on any change a consumer could notice (2: `run_finished` gained `incomplete` and `termination`; 3: the `search` command and its `search_results` event; 4: sub-agents — `agent_*` events, `agent_id` in `approval_requested`, `list_agent_profiles` and `reload_agent_profiles`; 5: turn limits removed — `agent_spawned` no longer has `max_turns`, `edit_applied` gains `agent_id` and `changeset_id`, `binary` in its files; 6: `edit_applied` files gain `absolute`, `patch` and `patch_truncated`, `tool_finished` gains `executed` and `status`, `agent_tool_finished` gains `executed`, `status`, `output` and `output_truncated`). Additions a consumer can ignore — new event types, new optional fields, new commands, new states such as `set_reasoning`, `reasoning_changed`, `effective_reasoning` or the skills events — keep the version; a removed or renamed field or event, or a changed meaning, increments it. Consumers ignore unknown types and fields |
| `session_id` | the session |
| `run_id` | the run (absent for session-level events) |
| `seq` | 1, 2, 3, … contiguous: a gap never happens silently |
| `at` | milliseconds since the Unix epoch |
| `type` | the event (below); its fields follow, flat |

Events (`type`):

| type | fields | |
|---|---|---|
| `session_opened` | `working_dir, model, reasoning?, effective_reasoning?, resumed, message_count, warnings` | first event; `reasoning`: the requested profile (absent: the model's default); `effective_reasoning`: the one that applies (absent when the model declares none) |
| `run_started` | `prompt, attachments[{kind,path,bytes,sha256?,note?}], model, reasoning?` | |
| `text_delta` | `text` | the answer, streamed |
| `thinking_delta` | `text` | only reasoning the provider exposed; never reconstructed |
| `tool_started` | `tool_call_id, name, input` | concurrent calls have distinct ids |
| `tool_progress` | `tool_call_id?, name, message` | long calls (shell) |
| `tool_finished` | `tool_call_id, name, is_error, duration_ms, output, executed?, status?` | sent as soon as the call is done, without waiting for the other calls of its round: several calls finish in the order they end. The history sent to the model keeps the order of the calls. `executed: false` for a call refused, denied or blocked before it ran; `status` is `success`, `failure`, `timed_out`, `cancelled` or `running` (a background command still going) |
| `approval_requested` | `approval{approval_id, tool_call_id, tool, level, description, input, preview?, agent_id?}` | `preview.files[{path, kind, before_sha256, diff, added, removed}]` |
| `approval_resolved` | `approval_id, tool_call_id, decision, by` | `decision`: `allow`, `allow_for_session`, `deny`; `by`: `user`, `session`, `non_interactive`, `cancelled` |
| `edit_applied` | `tool_call_id, tool, files[{path, kind, added, removed, binary?, absolute?, patch?, patch_truncated?}], agent_id?, changeset_id?` | a change was applied to the session's workspace: a successful `Write`, `Edit` or `ApplyPatch` of the session agent, or of a sub-agent working in that workspace (`agent_id`), or a ChangeSet applied (`changeset_id`); never for a denial, a refusal or a failure. Counts are those of the complete diff; one event per change |
| `memory_recalled` | `items, tokens, omitted, budget` | what went into the system prompt |
| `context` | `status{context_used{tokens, provenance, …}, context_window, input_limit, totals, …}` | `provenance`: `measured`, `counted`, `mixed`, `estimated` |
| `usage` | `turn{…}, total{…}` | observed token usage; `cost_usd` absent = unknown, not zero |
| `compaction` | `reason, outcome, compacted` | |
| `model_changed` | `model, reasoning?, effective_reasoning?, applies` | `next_turn` during a run, `next_run` otherwise |
| `reasoning_changed` | `model, requested?, previous?, previous_label?, effective?, effective_label?, applies` | answer to `set_reasoning`: the effective profile before and after (ids and configured labels); `requested` absent = the model's default |
| `context_cleared` | `messages_removed` | |
| `session_saved` | | |
| `notice` | `message` | retries, configuration diagnostics, continuations after a cut answer, no-progress warnings |
| `run_finished` | `outcome, failure?, error?, termination?, text, turns, approvals_unsatisfied?` | **exactly one per run**; `outcome`: `succeeded`, `incomplete`, `failed`, `cancelled`; `failure`: `approval_required`, `error`; `termination.kind`: `completed`, `output_truncated` (`continuations`), `no_progress` (`repeats`), `content_filtered`, `empty_response` (`max_turns` only in results stored before 0.4.8); `turns`: generation turns that got a response |
| `memory_maintenance_started` | | after `run_finished` |
| `memory_maintenance_finished` | `outcome, report?, error?` | `outcome`: `completed`, `cancelled`, `failed` |
| `search_results` | `query, status, hits[{path, line, column, text}], omitted, notes?, elapsed_ms` | answer to `search`; `line`/`column` 1-based, column in characters; `status`: `complete`, `partial` (a limit was reached: absence proves nothing), `cancelled`, `error` |
| `agent_spawned` | `agent{agent_id, parent_id?, root_run_id, tool_call_id?, batch_index?, background, depth, profile, profile_source, profile_revision, model{requested, applied, reason?}, reasoning{…}, workspace, isolation, branch?, task, created_at}` | a sub-agent was created (see `docs/agents.md`); `tool_call_id` is the parent's call |
| `agent_state` | `agent_id, state, reason?` | `queued`, `waiting_admission`, `starting`, `running`, `cancelling`, then one of `completed`, `incomplete`, `failed`, `cancelled`, `interrupted` (found unfinished when the session reopened) |
| `agent_tool_started` / `agent_tool_finished` | `agent_id, tool_call_id, name, input` / `…, is_error, duration_ms, executed?, status?, output?, output_truncated?` | the sub-agent's tool calls (its text is never streamed); `output` is cut to 64 KiB |
| `agent_finished` | `result{agent_id, profile, status, termination?, error?, summary, files_changed, commands, warnings, turns, usage, duration_ms, model, reasoning?, workspace, transcript?, changeset?, branch?, skills?}` | the compact result; its usage is not in the run's `usage` events (see `run_usage`) |
| `agent_profiles` | `profiles[{name, description, scope, source, valid, error?, shadows}], total, page, diagnostics?` | answer to `list_agent_profiles` / `reload_agent_profiles` |
| `changes_ready` | `changeset{id, agent_id, task, workspace, branch, base_sha, snapshot_id, baseline_tree, final_tree, files[{path, status, from?, added?, removed?, binary}], patch, patch_bytes, state, validations, note?}` | an isolated sub-agent ended with changes; nothing applied |
| `changes_updated` | `changeset_id, state, files, detail?` | `applied`, `conflict` (nothing written), `discarded` |
| `run_usage` | `root_run_id, own, descendants, total, final_total, pending_agents` | after a run, then once more when its last background descendant ends (`final_total: true`); each agent counted once |
| `agent_control_result` | `action, ok, text` | answer to `agent_control` |
| `job_started` / `job_output` / `job_finished` | `job_id, agent_id, root_run_id, command, cwd, pid` / `job_id, stdout_bytes, stderr_bytes` / `job_id, state, code?, signal?, duration_ms, logs` | background commands; `job_output` carries counts, never the output |
| `skill_activated` | `agent_id?, name, source, revision, origin` | a skill's instructions are in an agent's context; `origin`: `user`, `model`, `profile`, `restored`; `agent_id` absent for the session agent |
| `skill_activation_failed` | `agent_id?, name, reason` | not activated (refused, too large, invalid…), or suspended on restoration |
| `skills` | `listing{generation, project_decision, entries[{name, source, state, description?, path?, revision?, details}], total, page, diagnostics?, catalog_omitted}` | answer to `list_skills` / `reload_skills`; `state`: `available`, `active`, `shadowed`, `invalid`, `incompatible`, `pending_approval`, `refused`, `disabled` |
| `skills_trust_requested` | `project, sources[{source, path, skills}]` | project skills wait for a person's decision (interactive; once per session) |
| `skills_trust_resolved` | `project, allowed, by` | `by`: `user` or `caller` (`--trust-project-skills`) |
| `command_rejected` | `command, reason` | nothing changed |

A short run:

```text
{"schema":6,"session_id":"…","seq":1,"at":…,"type":"session_opened","working_dir":"/p","model":"demo/scripted","resumed":false,"message_count":0,"warnings":[]}
{"schema":6,"session_id":"…","run_id":"run_…","seq":2,"at":…,"type":"run_started","prompt":"Find the README","attachments":[],"model":"demo/scripted"}
{"schema":6,…,"seq":3,"type":"tool_started","tool_call_id":"call_0","name":"Glob","input":{"pattern":"*.md"}}
{"schema":6,…,"seq":4,"type":"tool_finished","tool_call_id":"call_0","name":"Glob","is_error":false,"duration_ms":3,"output":"README.md","executed":true,"status":"success"}
{"schema":6,…,"seq":9,"type":"text_delta","text":"I listed the Markdown files. …"}
{"schema":6,…,"seq":14,"type":"run_finished","outcome":"succeeded","termination":{"kind":"completed"},"text":"…","turns":3}
```

No API key, authentication header or secret appears in any event. The
views the frontends read do not carry them.

### Commands (for frontends)

```json
{"type":"submit","prompt":{"blocks":[{"kind":"text","text":"…"},{"kind":"file","path":"src/a b.rs"}]}}
{"type":"cancel"}
{"type":"set_model","model":"p/m","reasoning":"deep"}
{"type":"set_reasoning","profile":"medium"}
{"type":"set_reasoning"}
{"type":"compact"}
{"type":"clear_context"}
{"type":"resume","session_id":"…"}
{"type":"approve","approval_id":"ap_…","decision":"allow_for_session"}
{"type":"search","text":"needle","regex":false}
{"type":"list_agent_profiles","query":"review","page":0}
{"type":"reload_agent_profiles"}
{"type":"submit","prompt":{"blocks":[{"kind":"text","text":"…"}],"skills":["release-notes"]}}
{"type":"list_skills","query":"pdf","page":0}
{"type":"reload_skills"}
{"type":"trust_project_skills","allow":true}
{"type":"agent_control","action":"apply_changes","changeset_id":"cs_…"}
{"type":"agent_control","action":"stop_job","job_id":"job_…"}
```

`agent_control.action`: `list`, `status`, `result`, `cancel` (`agent_id`),
`inspect_changes`, `apply_changes`, `discard_changes` (`changeset_id`),
`jobs`, `stop_job` (`job_id`). Frontends see every instance and job of the
session.

Rules during a run:

* `cancel` is always accepted and reaches the run at once.
* `set_reasoning` selects a profile of the current model (`profile`: one of
  its ids, `default` included when a profile has that id), or goes back to
  the model's declared default (`profile` absent or null). An unknown id is
  refused (`command_rejected`) and nothing changes. `set_model` keeps its
  meaning: without `reasoning` it keeps the profile when the model is the
  same.
* `set_model` is accepted and applies from the next turn (`model_changed`
  says so). Profiles are the configuration's; there is no fixed list.
* `submit`, `resume`, `compact` and `clear_context` are refused
  (`command_rejected`). A second prompt never starts silently in
  parallel.
* `list_agent_profiles`, `reload_agent_profiles` and `agent_control` are
  accepted at any time; a reload does not change a running sub-agent.
  `cancel` while idle cancels pending background sub-agents.
* `list_skills`, `reload_skills` and `trust_project_skills` are accepted at
  any time; a reload does not change instructions already active. A
  `submit` whose `prompt.skills` cannot all be used is refused.
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
* when the consumer goes away (closed stdout, or a frontend that stopped),
  the run is cancelled (headless) or the session closes.

The durable session store is the authority on the conversation. The
frontends keep no conversation store of their own. On resume they read the
stored history.

## Approvals

The policy belongs to the engine and is the same for the desktop
application and the headless mode. It is configured in `[permissions]`:

* by tool name first (`[permissions.tools]`), then by permission level
  (`read_only`, `write`, `execute`, `dangerous`, `none`), then `default`;
* actions: `allow`, `ask`, `deny`;
* defaults: reading is allowed, writing and executing are asked;
  `forbidden` tools are always refused.

When a call must be asked about:

* **File changes** (`Write`, `Edit`, `ApplyPatch`). The tool
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

## The desktop application

The interactive interface is the desktop application, `apps/bricks-desktop`.
It drives the same engine contract as `bricks run`. The `/` commands below
have the same names in its command palette, which is built from the shared
registry (`bricks_agent::control::slash`).

* **Projects and conversations.** A project is a folder. The sidebar lists
  its conversations: choosing one opens it, and a new one starts in the
  project.
* **Resume.** `/resume` opens the session picker; `/resume <id>` resumes
  that session. Resuming takes the same path as `bricks run --session <id>`:
  the session's own folder and project, its model and reasoning profile and
  its compaction snapshots. See [Sessions and resume](#sessions-and-resume).
* **Approvals.** A pending approval shows the change before anything is
  written. The policy is the one of [Approvals](#approvals).
* **Runs and files.** Each run ends with a card: its duration, the files it
  changed (the sum of the applied edit operations, not a Git diff) and its
  commands, both closed by default. « Copier » copies the whole answer of a
  run. The « Lecteur » toggle opens a read-only reader in the right column,
  with one tab per file: Source, Aperçu for Markdown files, and Modifications
  (each recorded patch, never merged). A file name in the conversation opens
  it there; paths stay in the project folder or the agent workspace.

### Commands

`/model [p/m [profile]]` · `/search <text>` (`re:<regex>`) ·
`/agents [words | reload | running | status <id> | result <id> | cancel <id>]` ·
`/skills [words | reload | trust | deny]` · `/skill <name> <request>` ·
`/effort [id | default]` · `/changes <id> [inspect | apply | discard]` · `/jobs [stop <id>]` · `/memory` ·
`/context [details]` · `/cost [details]` · `/session` ·
`/resume [id]` · `/compact` · `/clear` · `/diff` · `/tools` · `/mcp` ·
`/config` · `/file <path>` · `/folder <path>` · `/image <path>` · `/help` ·
`/quit`.

* **Registry.** The registry is declarative (name, aliases, arguments,
  description, action). Engine actions are contract commands; the others
  open a view of the application.
* **`/diff`** shows three different things apart: the change waiting for a
  decision, the changes applied by the agent in this session, and the
  working tree's own Git state, which includes your changes.
* **`/config`** never shows keys or authentication headers.
* **`/effort`.** The reasoning profiles of the current model, as the
  providers file declares them (ids, labels, parameters); there is no
  built-in list. `/effort` lists them and `/effort <id>` selects one.
  `/effort default` goes back to the **model's declared default**: not
  "reasoning off", and not an earlier choice of the session. A profile
  whose id is `default` stays selectable. The change applies to the next
  request (during a run, its next turn); the request being streamed and its
  retries keep their selection. It is stored with the session, as the model
  is. An unknown id is refused with the choices, and nothing changes. A model
  without profiles says `No reasoning profiles configured for this model.`
* **`/cost`.** The session's usage from the engine: requests (compactions
  and requests without usage said apart), input, cache read, cache write,
  output and reasoning (a part of output, never added to it), and the
  pricing state: an estimate, `$0.00` for a known zero, `Not priced`, or
  `Not priced (partial)` with the known subtotal labelled as such and the
  unpriced categories. `/cost details`: per response received by the
  application since the session was opened or resumed (a resumed session's
  earlier requests are only in the total).
* **`/context`.** The active context apart from the session's consumption:
  window (`Not configured` when the configuration states none), prompt
  budget, current usage, occupation (`used / prompt budget`; may exceed
  100%), safe availability (`prompt budget − margin − upper bound`: what
  admission allows, prudent and not a server guarantee) and the source.
  `/context details`: measured and estimated parts, method, upper bound,
  output reserve, margin, raw availability (`prompt budget − used`),
  session totals and notes.
* **Edit counts.** The lines added and removed by the changes applied to the
  session's workspace since the session was opened: a sum of operations (two
  edits of one file both count; an edit undone by another counts twice), not
  a `git diff`. They come from `edit_applied` only: `Write`, `Edit`,
  `ApplyPatch` that succeeded, a sub-agent's in the same
  workspace, a ChangeSet applied (once). Not counted: denied, refused or
  failed calls, previews, files changed by `Bash`, MCP tools,
  `NotebookEdit` (no preview) or by hand, a worktree's writes before their
  ChangeSet is applied, binary files' lines. A new or resumed session starts
  from `+0 −0`.

### Attachments

* **What.** Files, folders and images come from the composer: `@`, or
  `/file`, `/folder`, `/image`.
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
and nothing else (no session: no line, exit code 0). Scripts that need every session use
`sessions --all --json`. A project's sessions are those whose
recorded folder is the same folder (canonical path: a symbolic alias
matches; `atlas` is not `atlas-old`; a sub-folder or another worktree is
another project). Older sessions without a recorded folder, and sessions
whose folder was deleted, appear only with `--all`, said so. Listing reads
only: no provider, model or key is needed, and nothing is created, moved
or changed. The session store is unchanged (one store, no copy per
project).

**Picker.** In the desktop application, `/resume` (alias `/sessions`)
without an id opens the session picker: the sessions of the open session's
project (after a resume, that session's project). The sidebar lists the
project's conversations. An empty list says so.

**Resuming another project's session** is allowed: `/resume <id>` and
`run --session <id>` take any id. It opens session B in
B's recorded folder with B's project — its `bricks.toml`, rules, system
prompt, long-term memory, profiles, CodeScout settings. It does not turn
the session you were in into a session of B, and `--workspace` does not
override B's folder. The session is read first: the settings of the
folder you started from are not loaded for it (a broken `bricks.toml`
there does not prevent resuming B). Everything B needs is prepared before
anything is switched: when that fails, the open session stays as it was
and the reason is shown. Approvals "for the session" stay with the
session they were given in; the previous session's background jobs stop.

`/resume` (with or without an id) and `bricks run --session <id>`
restore:

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

Shell state has its own life cycle. A resumed session starts with no running
commands: processes, background tasks and shell variables of the old one are
not restored.

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
```

## Tested platforms

* **macOS (arm64).** This is the only platform these tests ran on: the
  headless and JSONL modes, and a Ctrl+C in a pseudo-terminal
  (`pty_headless`, Unix-only).
* **Linux.** Not run.
* **Windows.** Not run: the headless mode builds without `/dev/tty`
  approvals (no prompt is offered). The desktop application is covered by
  its own checklist, not here.

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
* Images are attached by path.
* Two Bricks processes may open the same project or resume the same
  session at once; nothing locks between processes.
* A session's project cannot be changed while it is open (no
  `/workspace`); a workspace is one folder.
