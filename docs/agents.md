# Sub-agents and profiles

A Bricks agent can delegate one self-contained task to a **sub-agent** with
the native `Agent` tool. The sub-agent starts from a fresh context, works
with its parent's permissions and tools, and returns a compact result that
the parent integrates. No skill or external wrapper is involved: delegation
is a tool of the model.

Since Sprint 10.5 a parent can start **several sub-agents at once**
(`Agents`), in the **background** (a handle now, the result later), and a
sub-agent can start its own (bounded depth). Children that may write at the
same time work in **isolated git worktrees** started from the parent's
effective state (uncommitted changes included); their work comes back as a
**ChangeSet** that nothing applies without an explicit request. Long
commands become **jobs** (`Bash` with `background: true`, then `Job`).

| concept | what it is |
|---|---|
| tool | `Agent` (one child), `Agents` (several at once), `AgentControl` (handles and ChangeSets), `AgentProfiles`, `Job` (background commands) |
| profile | a specialisation: instructions and preferences (model, reasoning) |
| task | the goal of this one sub-agent |
| permissions | the parent's approval policy (`[permissions]`), unchanged |
| workspace | `shared` (the parent's directory) or `worktree` (isolated) |
| ChangeSet | an isolated child's work, as a patch to inspect, apply or discard |
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
(`auto`, `shared`, `worktree`), `background` (`true`: return a handle at
once). `prompt` is
accepted as an alias of `task` (the former `Agent` tool's field);
`system_prompt` is not (choose a profile).

The result returned to the parent is compact:

```text
Sub-agent agent_5f3a… (inspecteur, model p/m, reasoning high) — completed in 12345 ms, 7 turn(s), 9 tool call(s)

<the sub-agent's final answer, at most [agents] summary_chars>

Files changed: src/payment.rs
Commands run: `cargo test payment` (ok, 2310 ms)
Transcript: <session files>/agents/agent_5f3a….json
```

`status` is `completed`, `incomplete` (a stop before the answer: output, no
progress — not a success), `failed` or `cancelled`; a partial answer is kept.
Files changed come from applied, approved changes; commands from the shell
calls the sub-agent actually made, with their outcome — never from what the
model claims. The full conversation of the sub-agent is stored (JSON) and
referenced, never injected into the parent.

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
* `isolation`: `auto` (default), `shared`, `worktree`. `background`:
  `false` (default), `true`. The request's own value wins over the
  profile's (see *Isolation* and *Background agents* below).
* `max_turns` (former): turn limits were removed in 0.4.8. The key is still
  accepted, ignored, and reported as a diagnostic (remove it); nothing
  limits the child's turns.
* `skills`: skills loaded into the sub-agent's system prompt (see *Skills
  of profiles*); a missing skill is reported and takes no tool away.
* Parsing: `serde-saphyr` with a budget (depth 8, 256 nodes, 16 aliases,
  16 KiB of scalars, one document); unknown and duplicate keys are refused;
  tags are not executed; nothing is interpolated (`${HOME}` stays text).

### Built-in profiles

`crates/cersei-agent/agents/*.md`, parsed by the same parser. All use
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
  empty or cut answer. Depth, concurrency, per-run totals, admission and
  job rules are unchanged. A request with `max_turns` is refused with a
  migration message; nothing is started.

## Validation: before anything is built

Refused, with `Nothing was started.`, before any provider, shell, workspace
or request: an empty or invisible task (Unicode spaces and zero-width
characters included), an unknown or invalid profile, an unknown model or
reasoning id, a `max_turns` (removed), a `context` above the limit, an unknown
isolation, a depth or per-run total beyond the limits (`AgentDepthExceeded`,
`AgentTotalExceeded`), a cancelled run, an unknown field (`system_prompt`).
For `Agents`, every entry is validated first: one invalid entry starts
nothing.

## Permissions

* `Agent` and `Agents` are `execute` tools: the normal policy decides
  (allow, ask, deny in `[permissions]`, or a person). Allowing it once
  allows that delegation only; allowing `Agents` allows none of the
  children's own tools. Headless with the default rules, nobody can approve
  and the run ends with `approval_required`. `AgentControl` is read-only
  for reads, `execute` for `cancel`, `write` for applying or discarding.
* Every tool call of the sub-agent goes through the same policy and the same
  approval broker; approvals reach the parent's frontend with the
  sub-agent's `agent_id`. A session-wide approval applies to the session, as
  it does for the parent. `forbidden` stays forbidden.
* A profile cannot widen anything.

## Project of a session

A session's sub-agents use the project of the session's own folder (its
profiles, skills, `[agents]` settings), including after resuming a
session of another project (`docs/cli.md`, *Sessions and resume*). A
project folder (workspace) and a sub-agent's worktree are both folders:
a worktree is another workspace, and neither is an isolation of the
system.

## Context and services

The sub-agent receives: the engine's system prompt for its tools, its role
(a sub-agent with a fresh context), the profile's instructions, and a user
message made of the task and the explicit `context`. It does **not** receive
the parent's conversation, recalled long-term memory, tasks or anything not
passed explicitly. Its tools are rebuilt from the parent's tool factory
(never `cersei_tools::all()` as a fallback), with `Agent`, `Agents`,
`AgentControl` and `AgentProfiles` (within the depth limit) and never the
legacy `delegate`; its
shell is its own; it shares the workspace's CodeScout engine, the web
context and the MCP connections (not closed with it). Its system prompt and
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

* At most `[agents] max_batch` entries; each accepts the fields of `Agent`
  (except `background`, which applies to the whole call).
* The children run **concurrently**, within `max_concurrent` slots for the
  whole session. Results come back **in the order asked**, whatever the
  order they finish in; each `agent_spawned` carries the parent's
  `tool_call_id` and the entry's `batch_index`.
* A failure does not stop the siblings; `fail_fast: true` cancels them and
  reports each one's own state (`cancelled`, with its partial answer).
* A child that cannot get a slot within `admission_timeout_ms`, or finds the
  queue full (`max_queued`), gets its own `failed` result
  (`AgentQueueFull`, `AgentAdmissionTimeout`); the others go on.
* The legacy `delegate` / `run_batch` paths count against the same slots.

## Recursion and limits

A sub-agent has `Agent`, `Agents` and `AgentControl` too, up to
`max_depth` generations (the session's agent is 0). `max_total_per_run`
bounds the descendants of one top-level run, cumulatively. Refusals
happen before any provider is built.

**No circular wait.** A slot is held while an agent works, never while it
waits: an agent waiting for its children (`Agents`, `AgentControl wait`)
gives its slot back — and its writer admission — and takes them again
afterwards. A chain parent → child → grandchild completes with
`max_concurrent = 1`.

## Background agents and `AgentControl`

`background: true` (on `Agent` or `Agents`) returns a handle at once; the
parent's turn goes on and may end. The child keeps running and its events
keep coming (session stream); when it ends, its result is kept, not pushed
into the parent's conversation, and does not wake the parent.

`AgentControl` (`agent_id` / `changeset_id`):

| action | effect | level |
|---|---|---|
| `list` | instances visible to the caller | read-only |
| `status` | state, waits, slot, workspace | read-only |
| `result` | the kept `AgentResult` (idempotent; asks no model) | read-only |
| `wait` | wait (bounded by `timeout_ms`) for the end; a timeout cancels nothing | read-only |
| `cancel` | cancel it and its descendants | execute |
| `inspect_changes` | files and patch of a ChangeSet | read-only |
| `apply_changes` | apply it to the caller's workspace | write |
| `discard_changes` | drop it and its worktree | write |

A sub-agent sees only its own descendants; the session (and its frontends)
sees every instance. An id from another session gives nothing.

## Isolation: shared or worktree

`isolation: auto` (default, `[agents] default_isolation`) is `shared` for a
single foreground child and `worktree` for background children and for
`Agents` with more than one entry — whenever writers would really coexist.

* **shared**: the parent's directory. One writer at a time per workspace
  (writer admission): a writing call (`write`, `execute`, `dangerous`)
  waits while another agent holds it; read-only calls never wait.
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
* Refused with an explicit reason: not a git repository, no commit yet,
  submodules, branch or path collision, snapshot over its limits
  (2 000 untracked files, 5 MiB per file, 200 MiB in all), git older than
  2.17.

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

`Bash` with `background: true` starts a supervised task and returns a
`job_id` (and its working directory). `Job`:

| action | effect |
|---|---|
| `list` | your jobs |
| `status` | state, pid, readiness, bytes, cwd, kept logs |
| `output` | `stdout` / `stderr`, paged by line `offset` / `limit`, bounded in bytes |
| `wait` | until it ends or `timeout_ms` (a timeout stops nothing) |
| `stop` | TERM to the process group and tree, grace, then KILL; idempotent |

A job belongs to the agent that started it: another agent, run or session
sees nothing. Running is not ready: `ready_pattern` makes it ready, and
ready is still not proof the service works. Quota `[background] max_jobs`
is checked before anything starts. Output is drained continuously; memory
is bounded per stream and per line (a longer line is cut, marked), raw
logs are capped and copied to the session's files when the job ends. A
sub-agent's jobs stop when it ends; the session's when it closes; no job
outlives Bricks. Events `job_started`, `job_output` (throttled counts,
never the text), `job_finished`.

## Skills of profiles

A profile's `skills: [name, …]` are loaded with Bricks' skill loader from
the child's workspace (project, user, bundled) into its system prompt —
present on every request, never compacted away — within
`skills_max_bytes`. Each is reported in the result (`skills`: `loaded`,
`truncated`, `missing`, `invalid`, `skipped`) with its source and
revision. A missing skill is a diagnostic, not a failure; skills never
remove tools.

## Usage

Each agent's usage is its own. The top-level run reports `run_usage`:
`own`, `descendants`, `total` — each child counted once, by its own
provider responses — with `final_total: false` and `pending_agents` while
background descendants still run, then a final event when the last ends.
An unknown price stays unknown (no cost invented).

## Cancellation, headless, recovery

* Cancelling the parent's run cancels its foreground children (and their
  descendants) and waits, bounded, for their cleanup. Background children
  survive the turn, not the session: closing the session cancels them.
* **Headless** (`bricks run`): after the answer, background children are
  drained up to `background_drain_ms`, then cancelled; the run then exits
  with code **6** (background work not completed) — never a silent
  success.
* **Recovery.** Instances are recorded in `<session files>/agents/manifest.json`.
  On reopening, one that was not terminal is marked `interrupted`: nothing
  is restarted, no old pid is touched; its worktree and ChangeSet stay
  inspectable.

## Events (JSONL schema 5)

`agent_spawned` (identity, parent, root run, profile and its revision,
model and reasoning requested/applied, turns, workspace, task),
`agent_state` (`waiting_admission`, `starting`, `running`, `cancelling`,
then one of `completed`, `incomplete`, `failed`, `cancelled`, with a reason),
`agent_tool_started` / `agent_tool_finished` (the sub-agent's tool calls,
`duration_ms`), `agent_finished` (the compact result, the sub-agent's own
usage), `agent_profiles` (answer to `list_agent_profiles` and
`reload_agent_profiles`). Schema 5 (0.4.8): `agent_spawned` no longer has
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
summary_chars = 4000

max_concurrent = 8          # descendants working at once (session)
max_depth = 2               # generations below the session's agent
max_total_per_run = 32      # descendants per top-level run
max_batch = 8               # entries per Agents call
max_queued = 32             # children waiting for a slot
admission_timeout_ms = 600000
background_drain_ms = 600000   # headless wait for background children
default_isolation = "auto"  # auto | shared | worktree
skills_max_bytes = 24000

[agents.reasoning_aliases]
# high = "deep"

[background]
max_jobs = 16
output_buffer_bytes = 1048576   # per stream, in memory
max_line_bytes = 65536
raw_log_bytes = 52428800        # per raw log
stop_grace_ms = 2000
max_page_bytes = 65536          # largest Job output page
```

## Platforms

Developed and tested on macOS (git ≥ 2.17). Job process-tree stop relies
on process groups (Unix); Windows is not verified.

## Compatibility

The library type `AgentTool` (old `Agent` tool built from provider and tool
factories) and `delegate` / `run_batch` stay available for embedders, with
their Sprint 8 guarantees. Bricks' CLI and controller register only the
native tools, never two tools named `Agent`. The legacy paths go through
the same scheduler (slots and per-run totals), not a third architecture.
