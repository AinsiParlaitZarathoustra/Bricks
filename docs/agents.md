# Sub-agents and profiles

A Bricks agent can delegate one self-contained task to a **sub-agent** with
the native `Agent` tool. The sub-agent starts from a fresh context, works
with its parent's permissions and tools, and returns a compact result that
the parent integrates. No skill or external wrapper is involved: delegation
is a tool of the model.

A parent can start **several sub-agents at once** (`Agents`), and a
sub-agent can start its own (bounded depth). Sub-agents **always run in
parallel**: `Agent` and `Agents` return their ids at once, and the parent
follows them with `AgentControl` (`wait`, `result`, `message`, `resume`,
`cancel`). A sub-agent that has ended can be **resumed** under the same id,
and a running one can be sent a **message**. Children work in **isolated git
worktrees** by default (`isolation: auto`), started from the parent's
effective state (uncommitted changes included); their work comes back as a
**ChangeSet** that nothing applies without an explicit request. Long
commands become **jobs** (`Bash` with `background: true`, then `Job`).

| concept | what it is |
|---|---|
| tool | `Agent` (one child), `Agents` (several at once), `AgentControl` (ids, results, messages, resumes, ChangeSets), `AgentProfiles`, `Job` (background commands) |
| profile | a specialisation: instructions and preferences (model, reasoning) |
| task | the goal of this one sub-agent |
| permissions | the parent's approval policy (`[permissions]`), unchanged |
| workspace | `shared` (the parent's directory, one writer at a time) or `worktree` (isolated) |
| ChangeSet | an isolated child's work, as a patch to inspect, apply or discard |
| execution | one run of a sub-agent under its id: 1 at launch, one more for each resume |
| reasoning | a reasoning profile id of the chosen model |

Profiles **specialise, they do not restrict**: a `web_searcher` may edit a
file if the task and the policy allow it.

## Using it

The model calls:

```json
{"task": "Find where the payment total is computed and list the call sites, with evidence.",
 "profile": "inspecteur"}
```

Only `task` is required. Optional: `description` (a short label), `profile`,
`model` (`inherit`, `auto`, `provider_id/model_id`), `reasoning` (`inherit`
or an id of that model's profiles), `context` (extra context passed
explicitly, at most `[agents] max_context_chars`), `isolation`
(`auto`, `shared`, `worktree`). `prompt` is accepted as an alias of `task`
(the former `Agent` tool's field); `system_prompt` is not (choose a profile).

The `background` parameter is gone from the schemas, since every sub-agent
runs in parallel. A call that still sends it is accepted and the field is
ignored.

The call returns at once, with the id of the sub-agent. Its result is read
with `AgentControl` `wait` (until it ends, bounded by `timeout_ms`) and then
`result`. The result is compact:

```text
Sub-agent agent_5f3a… (inspecteur, model p/m, reasoning high) — completed in 12345 ms, 7 turn(s), 9 tool call(s)

<the sub-agent's final answer, at most [agents] summary_chars>

Files changed: src/payment.rs
Commands run: `cargo test payment` (ok, 2310 ms)
Transcript: <session files>/agents/agent_5f3a….json
```

`status` is `completed`, `incomplete` (a stop before the answer: output, no
progress — not a success), `failed` or `cancelled`; a partial answer is kept.
The answer is cut at `[agents] summary_chars` characters, the ellipsis
included, and the text says so. Files changed come from applied, approved
changes; commands from the shell calls the sub-agent actually made, with
their outcome — never from what the model claims. The full conversation of
the sub-agent is stored (JSON) and referenced, never injected into the parent.

## Profiles

### Where they come from

```text
<workspace>/.bricks/agents/*.md   (project)
  > ~/.bricks/agents/*.md         (user)
  > built-in profiles
```

The workspace is the session's working directory (the folder whose
`bricks.toml` applies); parent folders are not searched. A higher scope
shadows a lower one by name, and the listing shows what is shadowed. A
profile that is invalid, or a name defined twice in one scope, makes that
name unavailable with a diagnostic located on the file — it is **never
silently replaced** by a lower-priority profile of the same name. Other
profiles stay usable. At most 256 files per folder and 64 KiB per file are
read; there is no fixed number of custom profiles.

The registry is a snapshot made when the session opens; `/agents reload` (or
the `reload_agent_profiles` command) reads the files again. A sub-agent
already started keeps the profile it was started with (its revision is in
its `agent_spawned` event).

### Format

```markdown
---
name: inspecteur
description: >
  Localise précisément les modifications nécessaires et leurs preuves.
model: inherit
reasoning: high
permissions: inherit
tools: inherit
isolation: auto
background: false
skills: []
---

# Inspecteur

Instructions (the Markdown body, kept verbatim).
```

* The frontmatter opens the file (`---` on the first line; a BOM and CRLF
  are accepted) and closes on a line that is exactly `---`.
* `name`: 1–64 characters among `a-z 0-9 _ -`, starting with a letter or a
  digit. `description`: required, at most 1 024 characters.
* `model`: `inherit` (default), `auto`, `provider_id/model_id`.
* `reasoning`: `inherit` (default) or a reasoning profile id. Ids are the
  model's own (Bricks has no closed list of levels).
* `permissions`, `tools`: only `inherit` is implemented. A profile never
  widens or narrows the parent's permissions or tools; the policy is
  `[permissions]` in `bricks.toml`.
* `isolation`: `auto` (default), `shared`, `worktree` (see *Isolation*).
  `background`: `false` (default), `true`; the request's own value wins over
  the profile's.
* `max_turns` (former): turn limits were removed in 0.4.8. The key is an
  unknown key: it is ignored and reported as a diagnostic (remove it);
  nothing limits the child's turns.
* `skills`: skills preloaded whole into the sub-agent's system prompt (see
  *Skills of profiles*); one that cannot be used refuses the launch.
* Parsing: `serde-saphyr` with a budget (depth 8, 256 nodes, 16 aliases,
  16 KiB of scalars, one document); unknown and duplicate keys are refused;
  tags are not executed; nothing is interpolated (`${HOME}` stays text).

### Built-in profiles

`crates/bricks-agent/agents/*.md`, parsed by the same parser. All use
`model`, `permissions` and `tools: inherit`, `isolation: auto`,
`background: false`, with no business quota (files, tools, delegations).

| profile | for | reasoning |
|---|---|---|
| `orchestrateur` | splitting relevant parts, coordination, synthesis | high |
| `web_searcher` | dated sources, contradictions, documented conclusions | medium |
| `inspecteur` | an Edit Map: file/symbol/range, reason, references, tests, provenance, order | high |
| `backend_coder` | minimal changes to the engine, services and APIs | high |
| `frontend_coder` | UI/UX and integration, with the related backend changes | high |
| `testeur` | meaningful checks, observed results, failures, regressions | medium |
| `redactor` | documentation, specifications, plans, sourced syntheses | medium |

Without a profile, a neutral internal one is used. Examples of custom
profiles: [`docs/agents/examples/`](agents/examples/).

## Model and reasoning

Resolved when the sub-agent starts, and reported (`requested`, `applied`,
`reason`):

```text
request > profile > parent's current choice > model / Bricks default
```

* **Model.** `inherit` uses the parent's current model. `auto` uses
  `[agents] auto_model` when it is set and configured; otherwise the
  inherited model, with a warning. An explicit `provider_id/model_id` in the
  request must be configured, or the request is refused with the configured
  models; one in a profile that is not configured falls back to the
  inherited model with a warning.
* **Reasoning.** An explicit id in the request must be a profile of the
  chosen model, or the request is refused with that model's profiles. A
  profile's preference (`high`) absent from the model goes through
  `[agents.reasoning_aliases]` (`high = "deep"`) when one is configured,
  else the parent's profile if the model has it, else the model's default —
  with a warning. There is no "closest level" between free identifiers.
* **Turns.** No limit (0.4.8): a sub-agent works until its answer, like
  every agent. What still stops it: its answer, a cancellation (its
  parent's run, `AgentControl cancel`, the session), a definitive error, no
  progress (the same calls returning the same results), a refusal, an
  empty or cut answer. Depth, concurrency and admission apply as described
  below. A request with `max_turns` is refused as an unknown field, and
  nothing is started.

## Validation: before anything is built

Refused, with `Nothing was started.`, before any provider, shell, workspace
or request: an empty or invisible task (Unicode spaces and zero-width
characters included), an unknown or invalid profile, an unknown model or
reasoning id, a `context` above the limit, an unknown isolation, an unknown
field (`max_turns`, `system_prompt`), a depth beyond `max_depth`
(`AgentDepthExceeded`), a `worktree` launch outside a git repository (see
*Isolation*), a cancelled run.
For `Agents`, every entry is validated first: one invalid entry starts
nothing.

A child that starts and finds the queue full (`max_queued`), or no slot
within `admission_timeout_ms`, ends with its own `failed` result
(`AgentQueueFull`, `AgentAdmissionTimeout`); see *Several at once*.

## Permissions

* `Agent` and `Agents` are `execute` tools: the normal policy decides
  (allow, ask, deny in `[permissions]`, or a person). Allowing it once
  allows that delegation only; allowing `Agents` allows none of the
  children's own tools. Headless with the default rules, nobody can approve
  and the run ends with `approval_required`. `AgentControl` is read-only
  for `list`, `status`, `result`, `wait` and `inspect_changes`, `execute` for
  `message`, `resume` and `cancel`, `write` for applying or discarding.
* Every tool call of the sub-agent goes through the same policy and the same
  approval broker; approvals reach the parent's frontend with the
  sub-agent's `agent_id`. A session-wide approval applies to the session, as
  it does for the parent. `forbidden` stays forbidden.
* A change to a file (`Edit`, `Write`, `NotebookEdit`, `ApplyPatch`) is
  written under the process-wide lock of that file, so a parent and its
  children never overwrite each other's edits of one file. The approval
  is checked again under that lock: a file that moved on since it was
  approved is not written, and the call is decided again on its current
  state (see `docs/content/docs/built-in-tools.mdx`, *Edit*).
* A profile cannot widen anything.

## Project of a session

A session's sub-agents use the project of the session's own folder (its
profiles, skills, `[agents]` settings), including after resuming a
session of another project (`docs/cli.md`, *Sessions and resume*). A
project folder (workspace) and a sub-agent's worktree are both folders:
a worktree is another workspace, and neither is an isolation of the
system.

## Context and services

The sub-agent receives: the engine's system prompt for its tools (the Bricks
foundation, whose CodeScout and delegation bullets follow the child's own
tools), its role (a sub-agent with a fresh context), the profile's instructions, and a user
message made of the task and the explicit `context`. It does **not** receive
the parent's conversation, recalled long-term memory, tasks or anything not
passed explicitly. Its tools are rebuilt from the parent's tool factory
(never `bricks_tools::all()` as a fallback), with `Agent`, `Agents`,
`AgentControl` and `AgentProfiles` (within the depth limit); its shell is its
own; it shares the workspace's CodeScout engine, the web context and the MCP
connections (not closed with it). Its system prompt and
tools are not in its history: no compaction removes them.

## Several at once: `Agents`

```json
{"agents": [
  {"task": "Audit the payment module", "profile": "inspecteur"},
  {"task": "Find the provider's current rate limits", "profile": "web_searcher"},
  {"task": "Run the payment tests and report failures", "profile": "testeur"}
 ],
 "fail_fast": false}
```

* Each entry accepts the fields of `Agent`. There is no `background` field
  and no limit on the number of entries: children beyond the free slots wait
  in the queue (`max_queued`).
* The children run **in parallel**, within `max_concurrent` slots for the
  whole session. The call returns the ids of all of them, in the order asked,
  at once. Each result is read with `AgentControl` `wait` (and `result`) on
  its id. Each `agent_spawned` carries the parent's `tool_call_id` and the
  entry's `batch_index`.
* Every child goes through the same admission as a launch: a child that finds
  no free slot waits (`queued`, then `waiting_admission` for a shared
  workspace's writer). A child that finds the queue full (`max_queued`), or
  gets no slot within `admission_timeout_ms`, ends with its own `failed`
  result (`AgentQueueFull`, `AgentAdmissionTimeout`); the others go on.
* A failure does not stop the siblings. `fail_fast: true` cancels the others
  of the batch when one ends without completing; each reports its own state
  (`cancelled`, with its partial answer).
* Every entry is validated first: one invalid entry starts nothing.
* The library type `AgentTool` (for embedders) takes one slot of the same
  scheduler when it runs in a session (see *Compatibility*).

## Recursion and limits

A sub-agent has `Agent`, `Agents` and `AgentControl` too, up to
`max_depth` generations (the session's agent is 0, its children 1). A
launch deeper than that is refused before anything starts. There is no
cumulative limit on the descendants of a run: `max_total_per_run` is removed,
and the slots and the queue pace the work.

**No circular wait.** A slot is held while an agent works, never while it
waits: an agent waiting for its children (`Agents`, `AgentControl wait`)
gives its slot back — and its writer admission — and takes them again
afterwards. A chain parent → child → grandchild completes with
`max_concurrent = 1`.

## Handles, messages and resumes: `AgentControl`

`Agent` and `Agents` return the ids at once; the parent's turn goes on and
may end. A child keeps running and its events keep coming (session stream).
When it ends, its result is kept, not pushed into the parent's conversation,
and does not wake the parent: the parent reads it with `wait` or `result`.
Closing the session cancels the sub-agents still running.

`AgentControl` (`agent_id` / `changeset_id`):

| action | effect | level |
|---|---|---|
| `list` | instances visible to the caller | read-only |
| `status` | state, waits, slot, workspace | read-only |
| `result` | the kept `AgentResult` (idempotent; asks no model) | read-only |
| `wait` | wait (`timeout_ms`, default 60 000, at most 600 000) for the end; a timeout cancels nothing | read-only |
| `message` | queue a message for a running sub-agent (`agent_id`, `message`) | execute |
| `resume` | start a new execution of an ended sub-agent (`agent_id`, `prompt`) | execute |
| `cancel` | cancel it and its descendants | execute |
| `inspect_changes` | files and patch of a ChangeSet | read-only |
| `apply_changes` | apply it to the caller's workspace | write |
| `discard_changes` | drop it and its worktree | write |

A sub-agent sees only its own descendants; the session (and its frontends)
sees every instance. An id from another session gives nothing.

### Messages

`message` queues text for a running sub-agent. It reads the message at its
next model request, after the tool results pending at that point: a message
is never placed between a tool call and its result. A message that arrives
while the sub-agent writes its final answer makes it take one more request,
so it is read rather than dropped. Messages are read in the order sent.

At most 32 messages wait for one sub-agent, and one message is at most
16 KiB (UTF-8 bytes). The answer says what really happened; a refusal is
never reported as sent:

| outcome | answer |
|---|---|
| accepted | the message id, and how many messages wait |
| unknown, or out of your scope | no sub-agent with that id in your scope; nothing was sent |
| not running (ended, or not started yet) | its state; nothing was sent (`resume` continues an ended one) |
| empty (nothing but whitespace) | refused; nothing was sent |
| too large (over 16 KiB) | refused; nothing was sent |
| queue full (32 waiting) | refused; nothing was sent |

A sub-agent stopped by `cancel` refuses a later message; nothing resumes it
unless `resume` is asked for.

`SendMessage` is the same delivery under the built-in tool's name (see
`docs/content/docs/built-in-tools.mdx`). Without the agent runtime, nothing is
sent and the tool says so.

### Resumes

`resume` starts a new **execution** of an ended sub-agent under the same id.
Its stored conversation continues with `prompt` (required, not empty), with
the profile, model, reasoning, tools, skills and workspace of its **launch
record**: names, revisions and paths, never a secret value. The new execution
goes through the same admission as a launch and gets a new cancellation
token. The call returns at once; `wait` gives the new result. The stored
transcript keeps the active history and the raw history in full, across
executions.

`execution` counts 1 at launch and one more for each resume. The events of
an execution carry it (see *Events*); an event of an older execution is
ignored by the runtime and by the interface.

A resume is refused, before anything starts and with nothing changed, when:

* the `prompt` is empty;
* the id is unknown or out of your scope;
* the sub-agent is still running or queued (send it a `message` instead);
* another resume of the same id is already starting;
* it was launched before launch records existed: its result can be read, but
  it cannot be resumed faithfully;
* its profile changed since its launch (another revision), or is no longer
  available;
* one of its tools is no longer available;
* one of its skills changed or is no longer available;
* its stored conversation cannot be read;
* its changes were applied or discarded and its worktree is gone (nothing is
  brought back).

When its worktree still exists, the resume works in it. When it never
produced changes and its worktree is gone, a new worktree is made from a
snapshot of the parent's state.

## Isolation: shared or worktree

`isolation: auto` (default, `[agents] default_isolation`) gives each
sub-agent **its own git worktree**, since sub-agents run in parallel.
`shared` runs it in the parent's directory, and `worktree` asks for the
isolated checkout explicitly.

* **shared**: the parent's directory. One writer at a time per workspace
  (writer admission): a writing call (`write`, `execute`, `dangerous`) waits
  while another agent holds it, for at most `admission_timeout_ms`; read-only
  calls never wait. Use it for one writer at a time, or outside a git
  repository.
* **worktree**: `git worktree add -b bricks/<run>/<agent>` outside the
  project (`<session files>/agents/workspaces/worktrees/<agent>`), user
  hooks disabled. It starts from a **snapshot of the parent's effective
  state**: base commit + tracked changes (staged, unstaged, deletions,
  modes, binary) + untracked files git does not ignore. Taken read-only
  (`GIT_OPTIONAL_LOCKS=0`): the parent's index and files are never
  touched; no stash, reset, checkout or commit, and no commit is ever
  required first. Siblings of one batch share one snapshot.
* Relative paths of file tools resolve against the child's working
  directory (its worktree).
* Refused with an explicit reason, before anything starts: a worktree launch
  outside a git repository, no commit yet, submodules, branch or path
  collision, snapshot over its limits (2 000 untracked files, 5 MiB per file,
  200 MiB in all), git older than 2.17. Outside a git repository, the launch
  can run with `isolation: "shared"`, which works in place.

**Counted changes.** A child working in the session's workspace reports
each change it applies (`edit_applied` with its `agent_id`); the
interface adds them to its `+N −N`. A worktree's writes are not changes of
the session's workspace: they count once, when the ChangeSet is applied
(`edit_applied` with its `changeset_id`); `changes_ready`, inspecting,
a refused, conflicting or repeated apply, and discarding count nothing.

**ChangeSets.** When an isolated child ends with changes, they become a
ChangeSet (`changes_ready`): the diff from the worktree's **baseline**
(the snapshot applied), so what it inherited from the parent is never
counted as its work. Nothing reaches the parent's tree until
`apply_changes` (`/changes <id> apply`): `git apply --check` then
`git apply`, all or nothing. If the destination changed the same lines
meanwhile, it is a **conflict** (`WorktreeConflict`, files listed) and
nothing is written — never last-writer-wins. `discard_changes` removes the
worktree and its branch (only if the branch still points at its base). An
unchanged worktree is removed when its agent ends; a changed one is kept
until applied or discarded. No commit, push or PR is ever made, and
creating a local branch for a worktree does not authorize committing it.

A worktree is not a sandbox: processes, network, ports and databases stay
shared. `EnterWorktree` / `ExitWorktree` remain for people; `ExitWorktree`
never forces, and refuses the runtime's own worktrees.

## Background commands: jobs

`Bash` with `background: true` starts a supervised task and returns its
`task_id`, its `job_id`, its pid and its working directory. A foreground
command still running after `yield_time_ms` is handed over the same way. `Job`:

| action | effect |
|---|---|
| `list` | your jobs |
| `status` | state, pid, readiness, bytes, cwd, kept logs |
| `output` | `stdout` / `stderr`, paged by line `offset` / `limit`, bounded in bytes |
| `wait` | until it ends or `timeout_ms` (a timeout stops nothing) |
| `stop` | TERM to the process group and tree, grace, then KILL; idempotent |

A job belongs to the agent that started it: another agent, run or session
sees nothing. Running is not ready: `ready_pattern` makes it ready, and
ready is still not proof the service works. A place under `[background]
max_jobs` is reserved atomically before anything starts; beyond the limit
the start is refused, with nothing launched and no queue. Output is drained continuously; memory
is bounded per stream and per line (a longer line is cut, marked), raw
logs are capped and copied to the session's files when the job ends. A
sub-agent's jobs stop when it ends; the session's when it closes; no job
outlives Bricks. Events `job_started`, `job_output` (throttled counts,
never the text), `job_finished`.

## Skills of profiles

A profile's `skills: [name, …]` are resolved with the session's skills
service (`docs/skills.md`): the same sources, priorities and trust as the
session agent — a project skill needs the session's approval. They are read
and checked **before** the child is built and before any provider call:
a missing, refused, invalid or incompatible skill, or a total over
`skills_max_bytes` (24 000 bytes), refuses the launch with the reason;
nothing is cut. The skills go whole into the child's system prompt
(present on every request, never compacted away), are reported in the
result (`skills`: `loaded`, with source and revision) and as
`skill_activated` events (`origin: "profile"`, with the child's
`agent_id`). Older results may still show the former states (`truncated`,
`missing`, `invalid`, `skipped`).

Every child — parallel, nested, in a worktree — has its own
`Skill` tool and its own activations; it shares the session's sources and
trust, never the parent's activations or messages. A child in a worktree
reads its project's skills in place and works in its worktree. Skills
never remove tools or take the workspace's write lock.

## Usage

Each agent's usage is its own. The top-level run reports `run_usage`:
`own`, `descendants`, `total` — each child counted once, by its own
provider responses — with `final_total: false` and `pending_agents` while
sub-agents still run, then a final event when the last one ends.
An unknown price stays unknown (no cost invented).

## Cancellation, headless, recovery

* Cancelling the parent's run cancels its sub-agents still running (and their
  descendants) and waits, bounded, for their cleanup. Sub-agents survive the
  parent's turn, not the session: closing the session cancels them.
* **Headless** (`bricks run`): after the answer, the run waits for the
  sub-agents still running, for at most `[agents] background_drain_ms`
  (`600000` by default, counted from the end of the main run). What is still
  running then is cancelled, and the run exits with code **6** (sub-agent
  work not completed) — never a silent success. `0` cancels them at once.
* **Recovery.** Instances are recorded in `<session files>/agents/manifest.json`.
  On reopening, one that was not terminal is marked `interrupted`: nothing
  is restarted, no old pid is touched; its worktree and ChangeSet stay
  inspectable. A record that cannot be read is reported and left out, the
  others are kept, and the original file is kept as a
  `manifest.json.<time>.bak` backup before the next save replaces it.
* **Old ends.** A stored end whose kind this version does not know (for
  example `max_turns`, written before 0.4.8) is read as a historical end:
  incomplete, never a success, with its kind kept.

## Events (JSONL schema 6)

`agent_spawned` (identity, parent, root run, profile and its revision,
model and reasoning requested/applied, workspace, isolation, task),
`agent_state` (`queued`, `waiting_admission`, `starting`, `running`,
`cancelling`, then one of `completed`, `incomplete`, `failed`, `cancelled`,
`interrupted`, with a reason), `agent_tool_started` / `agent_tool_finished`
(the sub-agent's tool calls, `duration_ms`), `agent_finished` (the compact
result, the sub-agent's own usage), `agent_profiles` (answer to
`list_agent_profiles` and `reload_agent_profiles`).

`agent_state` and the `result` of `agent_finished` carry `execution` (1 at
launch, one more for each resume). These additions keep schema 6.

Schema 6 is additive: an event written at schema 5 reads as before, and an
absent `executed` means the call ran. `agent_tool_finished` gains `executed`
(`false` for a call that never ran: refused, denied, or blocked by a hook),
`status` (`success`, `failure`, `timed_out`, `cancelled`, or `running`) and
`output`, cut to 64 KiB (`output_truncated` says when). `edit_applied` files
gain `absolute` and `patch` (the unified diff actually written, cut to 512 KiB,
`patch_truncated`). `binary` is now set for a binary file, which has no patch
(it was always false before).

Schema 5 (0.4.8): `agent_spawned` no longer has
`max_turns`; `edit_applied` gains `agent_id` (a child's change in the
session's workspace) and `changeset_id` (a ChangeSet applied). Since 10.5
(additive, then schema 4):
`agent_spawned` gains `tool_call_id`, `batch_index`, `background`, `depth`,
`isolation`, `branch`; `agent_state` gains `queued` and `interrupted`;
`agent_finished.result` gains `changeset`, `branch`, `skills`; new
`changes_ready`, `changes_updated`, `run_usage`, `agent_control_result`,
`job_started`, `job_output`, `job_finished`; command `agent_control`. `approval_requested` carries `agent_id` for a
sub-agent's request. The sub-agent's text is never streamed as the parent's
answer, and its usage is not added to the parent's `usage` events (no double
count; unknown prices stay unknown).

## Durations

Durations shown in the terminal are in milliseconds everywhere, next to
their line: `Grep process_payment  12 ms`, `── done in 12345 ms`, never
switched to seconds. `<1 ms` means a measured positive duration under a
millisecond; `0 ms` a zero, or a value whose precision cannot tell; a call
replayed from a stored session has no duration (none is invented). While a
call runs, the interface shows the elapsed time it measures itself
(`… so far`). Stored values (`duration_ms`) and configured limits (timeouts
in seconds) are unchanged.

## Configuration (`[agents]` in `bricks.toml`)

```toml
[agents]
enabled = true              # register Agent and AgentProfiles
# auto_model = "provider/model"
max_context_chars = 16000
catalog_size = 30           # profiles described in the tool schema
summary_chars = 10000       # characters of a sub-agent's answer returned (exact, ellipsis included)

max_concurrent = 16         # sub-agents working at once in the session (launches, batches, resumes, any depth)
max_depth = 2               # generations below the session's agent
max_queued = 64             # children waiting for a slot (beyond: AgentQueueFull)
admission_timeout_ms = 60000   # a child waits this long for a slot or a writer, then AgentAdmissionTimeout
background_drain_ms = 600000   # headless: sub-agents still running get this long after the main run (exit 6 if cut)
default_isolation = "auto"  # auto (a worktree per sub-agent) | shared | worktree
skills_max_bytes = 24000

[agents.reasoning_aliases]
# high = "deep"

[bash]
yield_time_ms = 10000           # foreground command handed over as a task after this

[background]
max_jobs = 16
output_buffer_bytes = 1048576   # per stream, in memory
max_line_bytes = 65536
raw_log_bytes = 52428800        # per raw log
stop_grace_ms = 2000
max_page_bytes = 65536          # largest Job output page
```

`max_total_per_run` and `max_batch` are removed. As unknown keys they are
ignored, with a diagnostic; so are the other former keys of `[agent]` and
`[agents]` (`max_turns`, `default_max_turns`, `max_turns_cap`).

## Platforms

Developed and tested on macOS (git ≥ 2.17). Job process-tree stop relies
on process groups (Unix); Windows is not verified.

## Compatibility

The library type `AgentTool` (built from a provider factory and a toolset
factory; `ToolsetFactory` is in `bricks_agent::agent_tool`) stays available
for embedders, with its Sprint 8 guarantees: an empty task is refused before
anything is built, its children get the parent's permissions and at most its
tools, and never a delegation tool. When it runs in a session with a
scheduler, a child holds one slot; the library without a runtime is unchanged.

The CLI and the controller register only the native tools (`Agent`,
`Agents`, `AgentControl`, `AgentProfiles`), never two tools named `Agent`.

Removed in 14.6: the `delegate` tool, `DelegateTool`, the `delegate` module
(`run_batch`, `DelegateConfig`, `DelegateTask`, `DelegateResult`,
`DELEGATE_BLOCKED_TOOLS`, `DEFAULT_MAX_CONCURRENT`, `ProviderFactory`). A
call named `delegate` is now an unknown tool. The native `Agent` and `Agents`
tools have other parameters, and no equivalence is promised. Migration:
`docs/content/docs/migration.mdx`.
