# Sub-agents and profiles

A Bricks agent can delegate one self-contained task to a **sub-agent** with
the native `Agent` tool. The sub-agent starts from a fresh context, works
with its parent's permissions and tools, and returns a compact result that
the parent integrates. No skill or external wrapper is involved: delegation
is a tool of the model.

A parent can start **several sub-agents at once** (`Agents`), and a
sub-agent can start its own (bounded depth). Sub-agents **always run in
parallel**: `Agent` and `Agents` return their ids at once, and the parent
follows them with `AgentControl` (`wait`, `result`, `resume`,
`cancel`). A sub-agent that has ended can be **resumed** under the same id,
and a running one can be sent a **message**. Children work in **isolated git
worktrees** by default (`isolation: auto`), started from the parent's
effective state (uncommitted changes included); their work comes back as a
**ChangeSet** that nothing applies without an explicit request. Long
commands become **jobs** (`Bash` with `background: true`, then `Job`).

| concept | what it is |
|---|---|
| tool | `Agent` (one child), `Agents` (several at once), `AgentControl` (ids, results, resumes, ChangeSets), `SendMessage`, `AgentProfiles`, `Job` (background commands) |
| profile | a specialisation: instructions and preferences (model, reasoning) |
| task | the goal of this one sub-agent |
| permissions | the parent's permission policy, unchanged (see `docs/cli.md#permissions`) |
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
  widens or narrows the parent's permissions or tools; the policy is the
  parent session's (see `docs/cli.md#permissions`), not a section of
  `bricks.toml`.
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

* `Agent` and `Agents` are `execute` tools: the session's policy decides
  (ASK asks, EXECUTE allows, CUSTOM applies its execute rule or the rule of
  the tool). An answer covers that one delegation only, and allows none of
  the children's own tools. Headless under ASK, nobody can answer and the
  run ends with `approval_required`. `AgentControl` is read-only
  for `list`, `status`, `result`, `wait` and `inspect_changes`, `execute` for
  `resume` and `cancel`, `write` for applying or discarding.
* Every tool call of the sub-agent goes through the same engine and the same
  policy as the parent's (a sub-agent shares its parent's policy cell, so a
  change reaches it at its next call); approvals reach the parent's frontend
  with the sub-agent's `agent_id`. An answer covers one call, as for the
  parent. `forbidden` stays forbidden.
* A change to a file (`Edit`, `Write`, `ApplyPatch`) is
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
foundation, whose bullets follow the child's own tools), its role (a sub-agent
with a fresh context), the profile's instructions, and, assembled for each of its
requests, its own date, environment, git state, model and plan. A sub-agent with
TodoWrite has its own plan, in memory; it never gets the parent's. A user
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
* The CLI and the controller register only the native tools (`Agent`,
`Agents`, `AgentControl`, `AgentProfiles`), never two tools named `Agent`.

Removed in 14.6: the `delegate` tool, `DelegateTool`, the `delegate` module
(`run_batch`, `DelegateConfig`, `DelegateTask`, `DelegateResult`,
`DELEGATE_BLOCKED_TOOLS`, `DEFAULT_MAX_CONCURRENT`, `ProviderFactory`). A
call named `delegate` is now an unknown tool. The native `Agent` and `Agents`
tools have other parameters, and no equivalence is promised. Migration:
`docs/content/docs/migration.mdx`.
