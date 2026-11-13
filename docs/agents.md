# Sub-agents and profiles

A Bricks agent can hand a self-contained task to a **sub-agent**. It does this with the `Agent` tool. The sub-agent starts with a fresh context, uses the parent's tools and permissions, and sends back a short result.

Sub-agents always run in the background and in parallel. `Agent` returns their ids at once. Each report reaches the parent between two turns, when the sub-agent ends.

| Tool | What it does |
|---|---|
| `Agent` | Starts one sub-agent (`task`) or several (`missions`, 16 at most). |
| `AgentControl` | Lists agents, waits, reads results, resumes or cancels one, and handles ChangeSets. |
| `AgentProfiles` | Lists and searches the available profiles. |
| `Messagent` | Internal mail between the agents of one session. |
| `Job` | Reads and stops background shell commands (`Bash` with `background: true`). |

A profile is a specialty: instructions plus a model and reasoning preference. Profiles do not restrict anything. A `web_searcher` can edit a file if the task and the permissions allow it.

## Using `Agent`

```json
{"task": "Find where the payment total is computed and list the call sites, with evidence.",
 "profile": "inspecteur"}
```

Only `task` is required. Optional fields:

* `description`: a short label.
* `profile`: a profile name.
* `model`: `inherit` (default), `auto`, or `provider_id/model_id`.
* `reasoning`: `inherit` (default) or a reasoning id of the chosen model.
* `context`: extra text for the sub-agent (at most `[agents] max_context_chars`).
* `isolation`: `auto` (default), `shared`, or `worktree`.
* `missions`: a list of the fields above, to start several sub-agents in one call. Use it instead of `task`.

`system_prompt` is not accepted. Choose a profile instead. An empty task, an unknown profile or an unknown field is refused, and nothing is started.

To get an answer, call `AgentControl` with `wait` (it stops after `timeout_ms`, default 60000, and a timeout stops nothing). Then call `result`. A result looks like this:

```text
Sub-agent agent_5f3a (inspecteur, model p/m, reasoning high) - completed in 12 s, 7 turn(s), 9 tool call(s)

<the sub-agent's final answer>

Files changed: src/payment.rs
Commands run: `cargo test payment` (ok)
```

The status is `completed`, `incomplete`, `failed` or `cancelled`. The answer is cut at `[agents] summary_chars` characters, and the text says so. The list of files and commands comes from what the sub-agent really did, not from what it says.

An ended sub-agent can be resumed with `AgentControl resume` and a new `prompt`.

## What a sub-agent sees

It gets the system prompt for its tools, its profile instructions, its own date, environment and git state, and a message made of the task and the `context`. It does not get the parent's conversation or anything you did not pass in `context`.

Its shell is its own. It shares the workspace's code search and the MCP connections.

## Profiles

### Where they come from

```text
<workspace>/.bricks/agents/*.md   (project)
  > ~/.bricks/agents/*.md         (user)
  > built-in profiles
```

The workspace is the session's working directory. Parent folders are not searched. A profile in a higher scope hides one with the same name in a lower scope. A profile that is invalid, or a name defined twice in one scope, becomes unavailable and shows an error on its file. It is never silently replaced by a lower profile. At most 256 files per folder and 64 KiB per file are read.

Profiles are read when the session opens. `/agents reload` (or the `reload_agent_profiles` command, also used by the profiles view in Bricks Desktop) reads the files again. `/agents running` and `/agents status|result|cancel <id>` work on sub-agents that are running. A sub-agent that is already running keeps its profile.

### File format

```markdown
---
name: inspecteur
description: >
  Finds exactly which changes are needed, with evidence.
model: inherit
reasoning: high
permissions: inherit
tools: inherit
isolation: auto
background: false
skills: []
---

# Inspecteur

Instructions go here (the Markdown body, kept as written).
```

* The file starts with `---` and the header ends on a line that is exactly `---`.
* `name`: 1 to 64 characters among `a-z 0-9 _ -`, starting with a letter or a digit.
* `description`: required, at most 1024 characters.
* `model`: `inherit` (default), `auto`, or `provider_id/model_id`.
* `reasoning`: `inherit` (default) or a reasoning id of the model.
* `permissions` and `tools`: only `inherit` works. A profile never widens or narrows the parent's permissions or tools.
* `isolation`: `auto` (default), `shared`, or `worktree`.
* `background`: `false` (default) or `true`.
* `memory`: `off`, `read_only` or `read_write`. See `docs/memory.md`.
* `skills`: skills loaded whole into the sub-agent's system prompt. A skill that cannot be used stops the launch.
* An unknown key (for example the old `max_turns`) is ignored and reported. Duplicate keys are refused.

### Built-in profiles

They live in `crates/bricks-agent/agents/*.md`.

| Profile | For | Reasoning |
|---|---|---|
| `orchestrateur` | Splitting work, coordinating, summarizing | high |
| `web_searcher` | Dated sources, contradictions, sourced conclusions | medium |
| `inspecteur` | An edit map: file, symbol, reason, references, tests | high |
| `backend_coder` | Small changes to the engine, services and APIs | high |
| `frontend_coder` | UI changes and the backend changes they need | high |
| `testeur` | Useful checks, observed results, regressions | medium |
| `redactor` | Documentation, specs, plans | medium |

Without a profile, a neutral built-in one is used. Examples of custom profiles are in [`docs/agents/examples/`](agents/examples/).

## Model and reasoning

The first value that applies wins: the request, then the profile, then the parent's current choice.

* **Model.** `inherit` uses the parent's model. `auto` uses `[agents] auto_model` if it is set, otherwise the parent's model with a warning. An explicit model in the request must be configured, or the request is refused. An unconfigured model in a profile falls back to the parent's model with a warning.
* **Reasoning.** An id in the request must exist for the chosen model, or the request is refused. If a profile asks for a level the model does not have (`high`), Bricks uses `[agents.reasoning_aliases]` (for example `high = "deep"`), then the parent's level, then the model's default.
* **Turns.** There is no turn limit. A sub-agent works until it answers, is cancelled, fails, or makes no progress.

## Isolation

* `worktree`: the sub-agent works in its own git worktree, started from the parent's current files (uncommitted changes included). Its work comes back as a **ChangeSet**. Nothing is applied unless you ask. Use `AgentControl` with `inspect_changes`, `check_changes`, `apply_changes` or `discard_changes`.
* `shared`: the parent's folder, one writer at a time.
* `auto` (default): a worktree. Outside a git repository, choose `shared`.

## Permissions

* `Agent` is an `execute` tool and it never asks. Under EXECUTE it is allowed. Under CUSTOM it needs the `Agent` tool set to Allow. Under ASK it is refused and nothing starts.
* Every tool call of a sub-agent goes through the same permission policy as the parent's. Deny rules still hold.
* File changes (`Edit`, `Write`, `ApplyPatch`) take a lock on the file, so a parent and its children never overwrite each other.
* `AgentControl` is read-only for `list`, `status`, `result`, `wait` and `inspect_changes`. It needs `execute` for `resume` and `cancel`, and `write` for applying or discarding.

## Limits

Set them in the `[agents]` section of `bricks.toml` (see `docs/bricks.example.toml`):

| Key | Default | Meaning |
|---|---|---|
| `enabled` | `true` | Turn sub-agents on or off. |
| `auto_model` | none | The model that `auto` means. |
| `max_context_chars` | 16000 | Size limit for `context`. |
| `summary_chars` | 30000 | Size limit for a returned answer. |
| `max_concurrent` | 16 | Sub-agents working at once in a session. |
| `max_depth` | 1 | Levels below the main agent (1 means no grandchildren). |
| `max_queued` | 64 | Sub-agents waiting for a slot. |
| `admission_timeout_ms` | 60000 | How long a sub-agent waits for a slot. |
| `background_drain_ms` | 600000 | Headless runs: time given to running sub-agents after the main run. |
| `default_isolation` | `auto` | `auto`, `shared` or `worktree`. |
| `skills_max_bytes` | 24000 | Total size of profile skills loaded into a sub-agent. |

A sub-agent that finds the queue full, or gets no slot in time, ends with its own `failed` result (`AgentQueueFull`, `AgentAdmissionTimeout`). The others go on.

## Several at once

```json
{"missions": [
  {"task": "Audit the payment module", "profile": "inspecteur"},
  {"task": "Find the provider's current rate limits", "profile": "web_searcher"},
  {"task": "Run the payment tests and report failures", "profile": "testeur"}
]}
```

Every mission is checked first. One invalid mission starts nothing. A failure of one sub-agent does not stop the others.

## Messagent

`Messagent` is the internal mail of a session. The main agent is called `main`, and sub-agents have ids like `sub_0000`.

* `send`: write to `main`, to one or more ids, or to `team`. A message is at most 16 KiB.
* `read`: read new messages (`all: true` reads the whole thread).
* `members`: list who is in the team.

A message is read between two turns. It never interrupts a command, and it does not restart an ended sub-agent.
