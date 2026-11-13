# The `bricks` command

`bricks` is a small command line program. It reads your arguments and your configuration, runs one prompt or lists stored sessions, and prints what the engine (`bricks-agent`) reports. The engine does the real work: the agent loop, providers, tools, approvals, compaction and memory.

```text
bricks-cli (binary `bricks`)
  arguments, configuration, headless mode (text or JSONL)
        |  commands down, events up   (bricks_agent::control)
  engine: Controller -> Agent
        |
  providers, context, tools, approvals, memory, sessions

apps/bricks-desktop: the interactive interface, on the same contract
```

The engine never depends on `bricks-cli` or on any interface. Both frontends use the same contract (`bricks_agent::control`): typed commands go in, versioned events come out.

## Installation

```bash
cargo install --path crates/bricks-cli
```

This builds the `bricks` binary. The semantic code index uses USearch, a C++ library, so you need a C++17 toolchain. While developing, run it with `cargo run -p bricks-cli -- <arguments>`.

## Configuration

Two things are read. Nothing is built in.

* **Models**: `~/.bricks/providers.toml`, or `--providers <file>`, or `$BRICKS_HOME/providers.toml` when `BRICKS_HOME` is set (see `docs/providers.md`). A model is named `provider_id/model_id`. Its reasoning profiles, capabilities and prices come from this file.
* **Project**: `bricks.toml` in the project folder (the current folder, or `--workspace <dir>`). Sections: `[agent]`, `[agents]`, `[bash]`, `[background]`, `[semantic]`, `[memory]`, `[context]`, `[compression]`, `[web]` and `[[mcp.servers]]`. `docs/bricks.example.toml` lists the keys with their defaults. An invalid section falls back to its defaults, and a warning is shown when the session opens.
* **MCP servers** of your own are kept in `~/.bricks/mcp.toml` (see `docs/mcp.md`).
* **Permissions** are not read from `bricks.toml`. Each session has its own policy (see [Permissions](#permissions)). A `[permissions]` section in an old `bricks.toml` is reported and ignored: delete it.

```toml
[agent]
model = "my-provider/my-model"   # default model (otherwise --model is required)
reasoning = "deep"               # one of that model's reasoning profiles
```

Sessions are stored in `~/.bricks/sessions` (`$BRICKS_HOME/sessions`).

Bricks never picks a model for you. Without `--model` or `[agent] model`, `bricks` stops and lists the configured models.

## Projects (`--workspace`)

```bash
bricks sessions                                 # sessions of the current folder
bricks --workspace ~/Documents/atlas
bricks --workspace "~/Documents/My project"     # quoted: Bricks expands `~` itself
bricks --cd ./my-project                        # alias of --workspace
bricks --workspace ./my-project sessions        # global: before or after the command
bricks sessions --workspace ./my-project
bricks --workspace ./my-project run "Describe this project."
```

* **One option.** `--workspace` (alias `--cd`) is global. Giving it twice, under either name, is refused (exit code 2).
* **Resolution.** A leading `~` or `~/` is your home folder (also when quoted). `~user` and variables are not expanded. A relative path starts from the folder where Bricks was started. The result must be a readable folder. A missing folder or a file is refused before anything is created, stored or sent. Bricks never creates the folder.
* **What it sets.** The folder of a new session: its `bricks.toml`, rules, system prompt, memory, sub-agent profiles, relative tool paths and CodeScout. Bricks never changes the current folder of its own process.
* **Other paths.** `--providers` starts from the folder where Bricks was started. Attachments (`--file`, `--image`) are resolved against the session's folder.
* **Not a sandbox.** A workspace chooses settings and the scope of CodeScout. The other tools follow the approval policy.

## Tools of a session

A session of the `bricks` command or of Bricks Desktop has these tools:

* **Always**: `Read` (it also shows images), `Write`, `Edit`, `ApplyPatch`, `Glob`, `Grep`, `CodeScout`, `CodeStats`, `Bash`, `Job`, `PowerShell`, `WebFetch`, `WebSearch` and `TodoWrite` (the plan).
* **`CodeSearch`**: added when semantic search is set up (see `docs/semantic.md`).
* **With skills** (both frontends): `Skill`.
* **`Ask`**: lets the model ask you a question.
* **When `[agents] enabled` is true (the default)**: `Agent`, `AgentControl`, `AgentProfiles` and `Messagent`.
* **With `[memory] enabled = true`**: `memory_search` and `memory_write`.
* **MCP tools**, named `mcp__server__tool`.

Every call goes through the session's permission policy (see [Permissions](#permissions)). Each call is one of four classes (read, write, execute, network), and the policy allows it, asks, or refuses it. The built-in tools page of the site (`docs/content/docs/built-in-tools.mdx`) describes each tool.

## Commands

| command | what it does |
|---|---|
| `bricks` | prints the help. The command line has no interactive interface: use [the desktop application](#the-desktop-application) |
| `bricks run "..."` | run one prompt and print the answer |
| `bricks run --model <p/m> --reasoning <profile> "..."` | with another model or profile |
| `bricks run --json --non-interactive "..."` | JSONL on stdout, no human interaction |
| `bricks run --permissions ask\|execute "..."` | the permission profile of this run's session |
| `bricks run --session <id> "..."` | continue a stored session, in its own folder and project |
| `bricks run --skill <name> [--skill ...] "..."` | activate skills before the prompt (see `docs/skills.md`) |
| `bricks run --trust-project-skills ...` | allow project skills of configured sources, for this session only |
| `bricks sessions [--json]` | the project's stored sessions, most recent first |
| `bricks sessions --all [--json]` | every stored session, whatever the project |
| `bricks mcp list\|add\|test\|remove\|enable\|disable\|approve\|deny` | manage MCP servers (see `docs/mcp.md`) |

Global options: `--workspace <dir>` (alias `--cd`), `--providers <file>`, `--model <p/m>`, `--reasoning <profile>`.

## Headless mode (`bricks run`)

It works without a terminal and changes no terminal mode.

**Prompt and stdin.**

| invocation | prompt |
|---|---|
| `bricks run "text"` | `text`; stdin is not read, even when piped |
| `bricks run -` | stdin |
| `bricks run` (stdin not a terminal) | stdin |
| `bricks run` (stdin is a terminal) | error, exit code 2 (it never waits) |
| `bricks run "text" --stdin` | `text`, then stdin as a second block (`<stdin>...</stdin>`) |

`--file <path>` and `--image <path>` attach files and images (both can be repeated). Their content is captured when the run starts.

**Skills.** `--skill <name>` (repeatable) activates skills, in order, before the prompt. Every skill is checked before anything is sent. A skill that is missing, not approved, invalid or incompatible stops the command with exit code 2, and nothing is sent. Bricks reads the skills installed in its store (`<bricks_home>/skills`) and its built-in skill `create-skills`. It does not read `~/.claude/skills` or `.agents/skills`. `--trust-project-skills` only matters for project folders configured as explicit skill sources. None is configured by default. It grants no other permission. See `docs/skills.md`.

**Output.** Without `--json`, the answer is streamed to stdout. Tool calls, notices and errors go to stderr. With `--json`, stdout carries only JSONL events (schema below). Diagnostics go to stderr.

**Approvals.** When the policy says to ask, Bricks asks on the controlling terminal (`/dev/tty`) if there is one, and shows the diff of a file change: `Allow this call? [y]es / [N]o:`. The answer covers that call only. With `--non-interactive`, or without a terminal, nobody is waited for. The call is refused, and the run stops with `run_finished` `failure = "approval_required"` and exit code 3. `--non-interactive` grants nothing: calls the policy allows still run.

**Questions.** When the model uses `Ask`, the question is asked on the terminal too. With `--non-interactive` or without a terminal, it is skipped.

**Ctrl+C** cancels the run (or the memory maintenance after it). A second Ctrl+C exits at once. The session is stored. Tool calls already finished keep their result. Calls cut by the cancellation get a result saying so, so the session can be continued. Effects they already had stay: nothing is undone.

**Memory maintenance.** After the answer, the long-term memory (when enabled) processes the exchange. `bricks run` waits for it unless you pass `--no-wait-memory`. Unfinished work resumes at the next run.

### Exit codes

| code | meaning |
|---|---|
| 0 | the run succeeded |
| 1 | the run failed (provider, engine, tool infrastructure), or stdout closed |
| 2 | invalid arguments or configuration, unknown model or session, refused attachment: nothing ran |
| 3 | a step needed an approval and nobody could give it |
| 4 | the answer was delivered, but the memory maintenance failed |
| 5 | the run stopped at a limit before a final answer (`outcome = incomplete`); the partial answer was printed |
| 6 | the answer was delivered, but a background sub-agent did not complete (failed, cancelled, or still running when `[agents] background_drain_ms` ran out, `600000` by default). In text mode one line per agent goes to stderr. With `--json`, the `agent_finished` and `run_usage` events say it |
| 130 | cancelled (Ctrl+C) |

### When a run stops

A run ends when the model gives a final answer with no tool call left. Having tools is never an obligation to use them. Every other way a run can continue has a visible cause and a limit:

| continuation | cause shown | limit |
|---|---|---|
| tool calls | `tool_started` / `tool_finished` | no turn limit: the run goes on until its answer, one of the stops below, or Ctrl+C |
| answer cut by the output-token limit | `notice` "continuing (n/3)" | 3 continuations, then `incomplete` (`output_truncated`) |
| the same calls returning the same results | `notice` "No progress..." once, after 3 repeated rounds | 5 repeated rounds: `incomplete` (`no_progress`). Different arguments or results are progress |
| transport errors (429, 5xx, network) | `notice` "Retrying in ..." | 5 retries of one request with backoff, then `failed` |
| request refused as too long | `compaction` | `max_overflow_recoveries` of the context policy, per turn |

Old keys such as `[agent] max_turns`, `[agents] default_max_turns`, `max_turns_cap`, `max_batch` and `max_total_per_run` no longer exist. They are ignored and reported as a diagnostic when the session opens. Nothing is rewritten in your files.

When a run stops at a limit, the history, the partial answer and the tool results are kept, every tool call has a result, and no tool is started afterwards. The memory maintenance that follows never restarts the task.

If a provider's `Retry-After` is longer than `[agent] retry_after_max_wait_ms` (`60000` by default), the run does not wait. It fails at once with an error that names the setting. Shorter waits are taken, each shown by a `Retrying in ...` notice.

**Sub-agents** (the tools `Agent`, `AgentControl`, `AgentProfiles` and `Messagent`, active unless `[agents] enabled = false`; see `docs/agents.md`):

* `Agent` takes one mission, or a `missions` list. Each sub-agent runs in the background, and `Agent` returns the ids at once.
* Starting sub-agents follows the session profile. Under ASK it is refused. Under EXECUTE it runs. Under CUSTOM it runs only when the rule of `Agent` is `allow`. A launched sub-agent asks for nothing, and `deny` rules still hold.
* Sub-agents get their parent's permissions and at most its tools. They never get the sub-agent tools themselves.
* `AgentControl` (`wait`, `result`, `resume`, `cancel` and more) follows them. `Messagent` sends messages between agents.
* They are cancelled with the parent's run. In headless mode, sub-agents still running after the answer are waited for, at most `[agents] background_drain_ms`, then cancelled. Exit code 6 if any did not complete.
* Each ends as `completed`, `incomplete`, `cancelled` or `failed`, with its partial answer.

## The JSONL schema (version 9)

Every line is one envelope:

```json
{"schema":9,"session_id":"20261006-141502-a1b2c3","run_id":"run_5f...","seq":7,"at":1791300902123,"type":"tool_started","tool_call_id":"call_1","name":"Glob","input":{"pattern":"*.md"}}
```

| field | |
|---|---|
| `schema` | `9`. It goes up on any change a consumer could notice (a removed or renamed field or event, or a changed meaning). New event types, new optional fields and new commands keep the version. Consumers ignore unknown types and fields. Version 9 replaced the `Agents` and `SendMessage` tools with `Agent` (with `missions`) and `Messagent` |
| `session_id` | the session |
| `run_id` | the run (absent for session-level events) |
| `seq` | 1, 2, 3, ... with no gap |
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
| `approval_requested` | `approval{approval_id, tool_call_id, tool, level, description, input, preview?, agent_id?, policy_revision?}` | the engine said to ask. `preview.files[{path, kind, before_sha256, diff, added, removed}]`; `policy_revision`: the policy revision that asked |
| `approval_resolved` | `approval_id, tool_call_id, decision, by` | `decision`: `allow` (this call only), `deny`; `by`: `user`, `non_interactive`, `cancelled` |
| `question_asked` | `question{question_id, tool_call_id, question, options[], recommended?, timeout_seconds?, deadline_at?}` | the model used `Ask` and waits for an answer |
| `question_resolved` | `question_id, tool_call_id, outcome` | answered, or skipped (`user`, `timeout`, `cancelled`, `non_interactive`) |
| `permission_policy_changed` | `policy{profile, custom, chosen}, revision` | the session's policy was changed by a person; calls decided from now on follow `revision`. See [Permissions](#permissions) |
| `edit_applied` | `tool_call_id, tool, files[{path, kind, added, removed, binary?, absolute?, patch?, patch_truncated?}], agent_id?, changeset_id?` | a change was applied to the session's workspace: a successful `Write`, `Edit` or `ApplyPatch` of the session agent, or of a sub-agent working in that workspace (`agent_id`), or a ChangeSet applied (`changeset_id`); never for a denial, a refusal or a failure. Counts are those of the complete diff; one event per change |
| `memory_recalled` | `items, tokens, omitted, budget` | what went into the system prompt |
| `context` | `status{context_used{tokens, provenance, …}, context_window, input_limit, totals, …}` | `provenance`: `measured`, `counted`, `mixed`, `estimated` |
| `usage` | `turn{…}, total{…}` | observed token usage; `cost_usd` absent = unknown, not zero |
| `compaction` | `reason, outcome, compacted` | |
| `model_changed` | `model, reasoning?, effective_reasoning?, applies` | `next_turn` during a run, `next_run` otherwise |
| `reasoning_changed` | `model, requested?, previous?, previous_label?, effective?, effective_label?, applies` | answer to `set_reasoning`: the effective profile before and after (ids and configured labels); `requested` absent = the model's default |
| `context_cleared` | `messages_removed` | |
| `history_revised` | `user_index, messages_removed` | answer to `revise`: the active context was cut before the user message at `user_index` (its index among the user-role messages); the new run's `run_started` follows. The removed context is kept as a snapshot and in the raw history |
| `session_saved` | | |
| `title_changed` | `title` | the model named the session from its first prompt (the first title is that prompt, shortened); only when the frontend enables generated titles, and never over a rename |
| `plan_updated` | `plan{version, revision, todos[{content, status, activeForm}], explanation?, updated_at}` | the conversation's plan (TodoWrite) was saved: the complete state, whose `revision` orders it against the session state and other updates; `status`: `pending`, `in_progress`, `completed`; the envelope's `run_id` is the run whose tool call made the update, and the event comes before that call's `tool_finished`; never sent for a refused update |
| `notice` | `message` | retries, configuration diagnostics, continuations after a cut answer, no-progress warnings |
| `run_finished` | `outcome, failure?, error?, termination?, text, turns, approvals_unsatisfied?` | **exactly one per run**; `outcome`: `succeeded`, `incomplete`, `failed`, `cancelled`; `failure`: `approval_required`, `error`; `termination.kind`: `completed`, `output_truncated` (`continuations`), `no_progress` (`repeats`), `content_filtered`, `empty_response`; a kind stored by an earlier version (`max_turns`, before 0.4.8) is kept as stored and is a historical end, never `completed`; `turns`: generation turns that got a response |
| `memory_maintenance_started` | | after `run_finished` |
| `memory_maintenance_finished` | `outcome, report?, error?` | `outcome`: `completed`, `cancelled`, `failed` |
| `search_results` | `query, status, hits[{path, line, column, text}], omitted, notes?, elapsed_ms` | answer to `search`; `line`/`column` 1-based, column in characters; `status`: `complete`, `partial` (a limit was reached: absence proves nothing), `cancelled`, `error` |
| `agent_spawned` | `agent{agent_id, parent_id?, root_run_id, tool_call_id?, batch_index?, background, depth, profile, profile_source, profile_revision, model{requested, applied, reason?}, reasoning{…}, workspace, isolation, branch?, task, created_at}` | a sub-agent was created (see `docs/agents.md`); `tool_call_id` is the parent's call |
| `agent_state` | `agent_id, state, reason?, execution` | `queued`, `waiting_admission`, `starting`, `running`, `cancelling`, then one of `completed`, `incomplete`, `failed`, `cancelled`, `interrupted` (found unfinished when the session reopened) |
| `agent_text` | `agent_id, text` | what a sub-agent wrote in one turn, whole (not streamed) |
| `agent_tool_started` / `agent_tool_finished` | `agent_id, tool_call_id, name, input` / `…, is_error, duration_ms, executed?, status?, output?, output_truncated?` | the sub-agent's tool calls ; `output` is cut to 96 KiB |
| `agent_finished` | `result{agent_id, execution, profile, status, termination?, error?, summary, files_changed, commands, warnings, turns, usage, duration_ms, model, reasoning?, workspace, transcript?, changeset?, branch?, skills?}` | the compact result; its usage is not in the run's `usage` events (see `run_usage`) |
| `agent_profiles` | `profiles[{name, description, scope, source, valid, error?, shadows}], total, page, diagnostics?` | answer to `list_agent_profiles` / `reload_agent_profiles` |
| `changes_ready` | `changeset{id, agent_id, task, workspace, branch, base_sha, snapshot_id, baseline_tree, final_tree, files[{path, status, from?, added?, removed?, binary}], patch, patch_bytes, state, validations, note?}` | an isolated sub-agent ended with changes; nothing applied |
| `changes_updated` | `changeset_id, state, files, detail?` | `applied`, `conflict` (nothing written), `discarded` |
| `run_usage` | `root_run_id, own, descendants, total, final_total, pending_agents` | after a run, then once more when its last background descendant ends (`final_total: true`); each agent counted once |
| `agent_control_result` | `action, ok, text` | answer to `agent_control` |
| `job_started` / `job_output` / `job_finished` | `job_id, agent_id, root_run_id, command, cwd, pid` / `job_id, stdout_bytes, stderr_bytes` / `job_id, state, code?, signal?, duration_ms, logs` | background commands; `job_output` carries counts, never the output |
| `skill_activated` | `agent_id?, name, source, revision, origin` | a skill's instructions are in an agent's context; `origin`: `user`, `model`, `profile`, `restored`; `agent_id` absent for the session agent |
| `skill_activation_failed` | `agent_id?, name, reason` | not activated (refused, too large, invalid…), or withdrawn because its skill was removed, reinstalled or changed |
| `skills` | `listing{generation, project_decision, entries[{name, source, state, description?, path?, revision?, details}], total, page, diagnostics?, catalog_omitted}` | answer to `list_skills` / `reload_skills`; `state`: `available`, `active`, `shadowed`, `invalid`, `incompatible`, `pending_approval`, `refused`, `disabled` |
| `skills_trust_requested` | `project, sources[{source, path, skills}]` | project skills wait for a person's decision (interactive; once per session) |
| `skills_trust_resolved` | `project, allowed, by` | `by`: `user` or `caller` (`--trust-project-skills`) |
| `command_rejected` | `command, reason` | nothing changed |

A short run:

```text
{"schema":9,"session_id":"…","seq":1,"at":…,"type":"session_opened","working_dir":"/p","model":"demo/scripted","resumed":false,"message_count":0,"warnings":[]}
{"schema":9,"session_id":"…","run_id":"run_…","seq":2,"at":…,"type":"run_started","prompt":"Find the README","attachments":[],"model":"demo/scripted"}
{"schema":9,…,"seq":3,"type":"tool_started","tool_call_id":"call_0","name":"Glob","input":{"pattern":"*.md"}}
{"schema":9,…,"seq":4,"type":"tool_finished","tool_call_id":"call_0","name":"Glob","is_error":false,"duration_ms":3,"output":"README.md","executed":true,"status":"success"}
{"schema":9,…,"seq":9,"type":"text_delta","text":"I listed the Markdown files. …"}
{"schema":9,…,"seq":14,"type":"run_finished","outcome":"succeeded","termination":{"kind":"completed"},"text":"…","turns":3}
```

No API key, authentication header or secret appears in any event.

The state a frontend reads on demand (`Controller::snapshot`) has a `plan` field: the complete plan, with `revision` 0 before any update. It is the same state as the last `plan_updated`. A frontend treats a missing `plan` as no plan.

### Commands (for frontends)

```json
{"type":"submit","prompt":{"blocks":[{"kind":"text","text":"…"},{"kind":"file","path":"src/a b.rs"}]}}
{"type":"revise","user_index":1,"original":"Fix the parser","prompt":{"blocks":[{"kind":"text","text":"Fix the lexer"}]}}
{"type":"cancel"}
{"type":"set_model","model":"p/m","reasoning":"deep"}
{"type":"set_reasoning","profile":"medium"}
{"type":"set_reasoning"}
{"type":"compact"}
{"type":"clear_context"}
{"type":"resume","session_id":"…"}
{"type":"approve","approval_id":"ap_…","decision":"allow"}
{"type":"answer_question","question_id":"q_…","answer":{…}}
{"type":"set_permission_policy","policy":{"profile":"custom","custom":{"read":"allow","write":"ask","execute":"deny","network":"ask","tools":{"WebFetch":"allow"}},"chosen":true}}
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
* `revise {user_index, original, prompt}` replaces a past user message and
  runs again from it. `user_index` is the message's index among the
  user-role messages of the active history; `original` is its text, as the
  person saw it (compared trimmed). The active context is cut before that
  message: the context as it was is kept as a snapshot, and stays in the
  raw history. Then `history_revised` is sent and the run follows as for a
  `submit`. A skill activated only in the removed part is dropped from the
  session's ledger (a sub-agent profile's skills stay).
  Refused (`command_rejected`, nothing changed) while anything runs; when
  that message is not in the active history with that text (the history
  changed, or the message was compacted or cleared away, or it is a tool
  result); and for the reasons of `submit`.
* `list_agent_profiles`, `reload_agent_profiles` and `agent_control` are
  accepted at any time; a reload does not change a running sub-agent.
  `cancel` while idle cancels the sub-agents still running.
* `list_skills`, `reload_skills` and `trust_project_skills` are accepted at
  any time; a reload does not change instructions already active. A
  `submit` whose `prompt.skills` cannot all be used is refused.
* `set_permission_policy` is accepted at any time and answered by
  `permission_policy_changed`. It is stored with the session first. A
  request already waiting for a person is not answered by it; calls decided
  afterwards follow the new revision.
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


## Permissions

One engine decides every tool call, the same way in the desktop application
and in the headless mode. Each session has one **policy**: a profile and,
for the CUSTOM profile, its rules. Nothing is read from `bricks.toml`.

### Profiles

Every call falls in one of four classes: **read** (reads and searches of
the workspace), **write** (file changes), **execute** (commands and
sub-agents) and **network** (the web tools). A tool's permission level
gives its class; a `forbidden` tool is refused in every profile.

| profile | read | write | execute | network |
|---|---|---|---|---|
| `ask` | allowed | asked | asked | asked |
| `execute` | allowed | allowed | allowed | allowed |
| `custom` | the effect of its class, or of its tool | the same | the same | the same |

* **ASK** lets reads and searches run and asks about everything else.
* **EXECUTE** runs everything without asking.
* **CUSTOM** gives an effect (`allow`, `ask` or `deny`) per class and per
  tool. A tool's own rule wins over its class. It is the only profile
  that refuses calls outright.
* A session whose policy was never chosen behaves as ASK, and says so in
  the reason shown for each decision.

### Choosing a policy

* **Desktop application.** The general settings set the policy new conversations start from. While a
  conversation's policy is not chosen, a first-choice bar above the
  composer offers the profiles; the choice applies to that conversation and
  becomes the default. The `/config` view edits the conversation's policy,
  and `/tools` shows each tool's decision with its reason.
* **Command line.** `--permissions ask` or `--permissions execute` sets the
  profile of the run's session. Without it, the session keeps its own policy,
  or starts from the default chosen in Bricks (none chosen: as `ask`).
  CUSTOM rules are set in Bricks.

### Storage

* The default for new sessions is stored in `<bricks_home>/permissions.json`
  (`~/.bricks/permissions.json`), chosen once. An unreadable file is an error
  and never a silent allow: the session starts with no policy chosen (as
  ASK) and a warning says why.
* Each new session copies that default into its own `permissions.json`, in
  its files folder, and keeps it: a later change of the default does not
  reach it. A person can change a session's policy at any time. A stored
  session policy that cannot be read stops the session from opening.
* Sub-agents use their session's policy. They never get more.

### Revisions

Each change of a session's policy gets a new revision. A verdict records the
revision it was made under (`approval.policy_revision` in the events). A
request already waiting for a person is not answered by a change: calls
decided after the change follow the new revision.

### Migrating from `[permissions]`

The `[permissions]` section of `bricks.toml` is no longer read. Its rules,
denials included, are not applied; the file reports it as a diagnostic.
Delete the section, then choose the profile in Bricks (or pass
`--permissions` on the command line). The rules you need are set again as
CUSTOM rules, per class and per tool, in the conversation's `/config` view or
in Settings.

### Approvals

When a call the policy says to ask about is waiting:

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
* **An answer covers that call only.** There is no "allow for the session":
  a later call of the same tool is decided by the policy again. To allow a
  tool for good, change the policy (a CUSTOM rule for the tool).


## The desktop application

The interactive interface is the desktop application, `apps/bricks-desktop` (Tauri 2, Svelte 5 and shadcn-svelte, in English and French). It drives the same engine contract as `bricks run`. The `/` commands below have the same names in its command palette, which is built from the shared registry (`bricks_agent::control::slash`).

* **Projects and conversations.** A project is a folder. The sidebar lists its conversations. Choosing one opens it, and a new one starts in the project.
* **Provider keys.** Keys are kept in `providers.toml` (see `docs/providers.md`).
* **Resume.** `/resume` opens the session picker. `/resume <id>` resumes that session, the same way as `bricks run --session <id>` (see [Sessions and resume](#sessions-and-resume)).
* **Approvals.** A pending approval shows the change before anything is written. You can allow this call or refuse it. The policy is the one of [Permissions](#permissions).
* **Questions.** When the model uses `Ask`, the question appears in the conversation. You can pick an option, type your own answer, or skip.
* **Runs and files.** Each run ends with a card: its duration, the files it changed (the sum of the applied edits, not a Git diff) and its commands. A reader in the right column shows the files, with the source, a preview for Markdown, and the recorded patches.
* **Images.** A successful `Read` of an image shows a thumbnail under its line.
* **Diagrams.** The conversation renders SVG and Vega-Lite diagrams.
* **Plan.** A block under the conversation shows the plan (`TodoWrite`): done out of total, and the steps.
* **Memory view.** `/memory` shows the stored notes. You can delete an entry (see `docs/memory.md`).
* **Context breakdown.** `/context` shows how the context window is used (see below).
* **Search.** The magnifier in the sidebar searches the titles and messages of every project. It is not the `/search` command, which searches the text of the workspace.
* **Editing a sent message.** You can edit a message you sent and run again from it (not for typed commands such as `/skill ...`, and not while a run works). This sends a `revise` command. The replaced run records are moved to `.revised.jsonl` files, not deleted.
* **Projects with a missing folder.** The project stays in the list with a red icon. Opening it asks you to choose the new folder, and its conversations follow. The move is refused while one of its conversations runs, and when the folder belongs to another project. `bricks run --session` never moves a project.

### Commands

`/model [p/m [profile]]` · `/search <text>` (`re:<regex>`) · `/agents [words | reload | running | status <id> | result <id> | cancel <id>]` · `/skills [words | reload | trust | deny]` · `/skill <name> <request>` · `/effort [id | default]` · `/changes <id> [inspect | apply | discard]` · `/jobs [stop <id>]` · `/memory` · `/context [details]` · `/cost [details]` · `/session` · `/resume [id]` · `/compact` · `/clear` · `/diff` · `/tools` · `/mcp` · `/config` · `/file <path>` · `/folder <path>` · `/image <path>` · `/help` · `/quit`.

Short aliases: `/m` (model), `/s` (search), `/ctx` (context), `/usage` (cost), `/sessions` (resume), `/img` (image), `/?` (help), `/exit` and `/q` (quit).

* **Registry.** Each command has a name, aliases, arguments, a description and an action. Engine actions are contract commands. The others open a view of the application.
* **`/diff`** shows three separate things: the change waiting for a decision, the changes the agent applied in this session, and the Git state of the working tree.
* **`/config`** never shows keys or authentication headers.
* **`/mcp`** shows the MCP servers and their state. Manage them with `bricks mcp` or in the application.
* **`/effort`.** The reasoning profiles of the current model, as `providers.toml` declares them. `/effort` lists them and `/effort <id>` selects one. `/effort default` goes back to the model's declared default. It is not "reasoning off". The change applies to the next request and is stored with the session. An unknown id is refused and nothing changes. A model without profiles says `No reasoning profiles configured for this model.`
* **`/cost`.** The session's usage: requests, input, cache read, cache write, output and reasoning (a part of output, never added to it), and the price state: an estimate, `$0.00` for a known zero, `Not priced`, or `Not priced (partial)`. `/cost details` lists each response received since the session was opened or resumed.
* **`/context`.** The active context: window, prompt budget, current usage, and what is safely left. `/context details` splits the measured and estimated parts and shows the output reserve, the margin and the session totals.
* **Edit counts.** The lines added and removed by the changes applied to the session's workspace. It is a sum of operations, not a `git diff`. It counts successful `Write`, `Edit` and `ApplyPatch` calls, a sub-agent's edits in the same workspace, and an applied ChangeSet. It does not count refused or failed calls, files changed by `Bash`, MCP tools or by hand, or binary files.

### Attachments

* **What.** Files, folders and images come from the composer: `@`, or `/file`, `/folder`, `/image`.
* **Capture.** A file's content is captured when the prompt is sent. Its path, size and SHA-256 go into the message, and the session keeps exactly what was sent. A later edit of the file does not change the history.
* **Folders** become a bounded listing (300 entries, depth 4, `.gitignore` respected), never their full contents.
* **Images** are sent as image blocks. The command is refused before sending when the model does not declare image input.
* **Paths** are resolved against the working directory when the prompt is sent. A missing path refuses the submission, so nothing is sent.

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
B's recorded folder with B's project: its `bricks.toml`, rules, system
prompt, long-term memory, profiles, CodeScout settings. It does not turn
the session you were in into a session of B, and `--workspace` does not
override B's folder. The session is read first: the settings of the
folder you started from are not loaded for it (a broken `bricks.toml`
there does not prevent resuming B). Everything B needs is prepared before
anything is switched: when that fails, the open session stays as it was
and the reason is shown. Each session keeps its own permission policy: the
one of B is B's, and the previous session's background jobs stop.

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
to the current folder. In the desktop application, a project whose folder
disappeared can be moved to its new folder, and its conversations follow it
(see [The desktop application](#the-desktop-application)); this command does
not move anything. No replacement is chosen silently; `--model` picks
one explicitly. An older session without recorded settings resumes in the
current project folder with the selected model, and a warning says which.

Shell state has its own life cycle. A resumed session starts with no running
commands: processes, background tasks and shell variables of the old one are
not restored.

Deleting a session (`Memory::delete`) removes its history, raw history,
snapshots, saved tool outputs, `session.json` (its settings) and its files
folder, which holds the plan (`plan.json`).

## Long-term memory

With `[memory] enabled = true` (see `docs/memory.md`), each run's prompt
recalls stored facts (`memory_recalled`). After the answer, the exchange
is processed as a separate, cancellable phase
(`memory_maintenance_started` / `_finished`). That phase is durable and
resumable: an interrupted pass leaves its work pending, and a successful
extraction is never repeated. A maintenance failure is reported on its
own; it never turns into a "memory success", and the delivered answer is
unaffected.


## Long-term memory

With `[memory] enabled = true` (see `docs/memory.md`), each run's prompt recalls stored notes (`memory_recalled`). After the answer, the exchange is processed as a separate, cancellable phase (`memory_maintenance_started` / `_finished`). An interrupted pass leaves its work pending, and a finished extraction is never repeated. A maintenance failure is reported on its own. It never changes the answer you already got.

## A demonstration without any key

`scripts/demo_model.py` is a local scripted model server (Python standard library). It speaks `chat_completions` on 127.0.0.1:8765, with `scripts/demo_providers.toml`.

```bash
python3 scripts/demo_model.py &
export BRICKS_HOME="$PWD/.demo-home"
printf '[agent]\nmodel = "demo/scripted"\n' > bricks.toml   # in a scratch directory
bricks --providers scripts/demo_providers.toml run --json --non-interactive "Find the README"
bricks --providers scripts/demo_providers.toml run --non-interactive "please write a file"; echo "exit $?"   # 3: approval needed
bricks --providers scripts/demo_providers.toml sessions
```

## Tested platforms

* **macOS (arm64).** The only platform these tests ran on: the headless and JSONL modes, and Ctrl+C in a pseudo-terminal.
* **Linux and Windows.** Not run. On Windows the headless mode builds without `/dev/tty` approvals, so no prompt is offered.

## Known limits

* Tool progress events carry the tool's name, not its call id.
* A sub-agent's events carry its `agent_id`, its parent, the root run and the parent's `tool_call_id`.
* Two Bricks processes may open the same project or resume the same session at once. Nothing locks between processes.
* A session's project cannot be changed while it is open (there is no `/workspace`).
