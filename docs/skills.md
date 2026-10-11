# Skills

Bricks reads skills in the [Agent Skills](https://agentskills.io/specification)
format. A skill is a directory holding a `SKILL.md` — YAML frontmatter, then
Markdown instructions — and, optionally, other files. Loading a skill gives the
agent instructions and the list of its files. It runs no command, starts no
agent and grants no permission. Every action still goes through the agent's
tools and the approval rules of `bricks.toml`.

The service is the `bricks-skills` crate. The `Skill` tool, the session agent,
sub-agents, the CLI, the interface and the SDK all use it.

## Format

```markdown
---
name: release-notes            # required
description: Draft release notes for the changes since the last tag. Use when …   # required
license: MPL-2.0               # optional
compatibility: Needs a Git repository.   # optional, ≤ 500 characters
metadata:                      # optional: strings to strings
  team: docs
allowed-tools: Bash(git:*) Read   # optional (experimental): kept, not applied
---
# Release notes

1. …
```

* **`name`**: 1–64 characters (characters, not bytes), lowercase letters, digits
  and `-`; Unicode letters are allowed. It cannot start or end with `-` or contain
  `--`, and must equal the name of its directory. Both are compared after NFKC
  normalisation, as in the reference validator `skills-ref` (rules checked against
  its source at revision `69ef37e`).
* **`description`**: 1–1024 characters. It says what the skill does and when to use
  it; it is the only part the model sees before activation.
* `license`, `compatibility` and `metadata` are kept. `metadata` values must be
  scalars; numbers and booleans are kept as strings.
* **`allowed-tools`** is kept and shown on activation, with the statement that Bricks
  neither grants nor restricts anything because of it.
* Delimiters are whole `---` lines; LF and CRLF are accepted, and a BOM is ignored.
  The YAML is parsed under a budget (depth, nodes, aliases, 32 KiB of scalars, one
  document). Duplicate keys and tags are refused. Nothing is repaired: a broken
  document is a diagnostic.
* **Bricks' limits**, which are not part of the standard: a `SKILL.md` of at most
  100 KiB and a frontmatter of at most 32 KiB.

### Fields Bricks does not implement

* Informational fields outside the standard (`version`, `argument-hint`, `tags`,
  `platforms`…) produce a note and are ignored.
* Claude Code directives that would change how a skill runs, is activated or what it
  may do — `context`, `agent`, `model`, `effort`, `hooks`, `paths`, `shell`,
  `disable-model-invocation: true`, `user-invocable: false` — make the document
  **incompatible**. Such a skill can be inspected (`/skills`) but is neither activated
  nor preloaded by a profile; convert it first (remove the directive, keeping only
  what Bricks supports). `disable-model-invocation: false` and `user-invocable: true`
  are their defaults and change nothing.
* Nothing is substituted or executed: `$ARGUMENTS` and `` !`command` `` stay text
  (a note says so). A request is given separately (see *Activation*).

Finding `.claude/skills` means Bricks reads the standard documents there. It is not
full compatibility with Claude Code skills.

## Where skills come from

By priority (the first source wins a name):

| # | Source | Scope |
|---|---|---|
| 1 | `<project>/.agents/skills/<name>/SKILL.md` | project |
| 2 | `<project>/.claude/skills/<name>/SKILL.md` | project |
| 3 | `~/.agents/skills/<name>/SKILL.md` | personal |
| 4 | `~/.claude/skills/<name>/SKILL.md` | personal |
| 5 | built into Bricks | built-in |

* Only these folders are read, one level deep. Bricks does not walk parent folders,
  sub-projects or the home folder, and has no `.bricks/skills`.
* A missing folder is normal. An unreadable or invalid `SKILL.md` is a diagnostic.
* Symbolic links (a linked skill directory, a linked `SKILL.md`) are not followed;
  the listing says so.
* The scan is bounded (512 entries per folder, 1024 candidates). The order of the
  file system never changes the result.
* **Collisions** are visible in `/skills`: the winner, and below it the homonyms it
  hides (`shadowed`).
* Among the sources you allow, an invalid or incompatible winner makes its name fail.
  There is no silent fallback to a lower homonym. A document whose frontmatter does
  not parse is keyed by its directory's name, so the conflict is still seen.
* A project source you have not approved is ignored by resolution. It provides
  nothing, and cannot block a personal or built-in skill.
* Embedders (SDK) can add sources with `SkillRoot::explicit(id, path, trusted)`
  (`SkillsSetup::extra_roots`), after the default ones. A path is never trusted
  because a project file asked for it.

Built-in skills: `commit`, `debug`, `remember`, `simplify`, `stuck`, `verify`.
They are standard documents compiled into the binary and read by the same parser.
`commit` describes a procedure and commits nothing by being activated. The user's
instructions and the repository's rules come first. There is no `loop` skill:
Bricks schedules nothing.

Sources are read when a session opens and again on request (`/skills reload`,
`reload_skills`), never on every turn. A reload does not change instructions that
are already active.

## Trust of project skills

A project's skills (sources 1 and 2) are usable only after an explicit decision,
taken for that project and that session:

* **Interactive**: Bricks reports what it found (`skills_trust_requested`, asked once
  per session). `/skills` lists those skills for you to inspect. Then `/skills trust`
  allows them and `/skills deny` refuses them.
* **Headless**: `bricks run --trust-project-skills …` allows them for that session
  only. Without it, a notice says they are not used. Nothing waits for an answer.
* **SDK**: `SkillsSetup::trust_project` when the session is opened,
  `Controller::trust_project_skills(bool)`, or `SkillAccess::decide_project(…)` for an
  agent built directly.

Until then, no name, description or instruction of the project's skills reaches the
model. The model cannot approve them (the `Skill` tool has no such action), and
neither can a file of the repository, `bricks.toml` or a skill.

A refusal leaves the session usable with the other sources, and Bricks does not ask
again in that session.

The decision is not stored as a blanket permission. A resumed session starts
undecided, as does a change of project (`/resume` of another project's session).

Personal and built-in skills are usable without asking. That is the local policy:
an embedder can turn personal skills off (`SkillsSetup::personal = false`).

Trust lets the model *read* the project's procedures. It approves nothing they
describe: `Write`, `Bash`, MCP, network and delegation keep their own approvals.

## Activation

* **By the model.** The system prompt carries a catalog of the skills it may activate
  (name and description only, escaped data in one `<available_skills>` block). The
  catalog is bounded (100 entries, 16 KiB) and says how many skills it leaves out;
  `Skill` with `action: "list"` and a `query` finds them. The model activates one
  with `Skill` (`action: "activate"`, `skill`, and optionally `args`: the request,
  returned separately, never substituted into the text). `skill: "list"` without an
  `action` still lists, so a skill really named `list` is activated with
  `action: "activate"`.
* **By a person.** `/skill <name> <request>` in the interface, or `bricks run --skill
  <name> …` (repeatable). Each skill is checked before anything is sent and placed in
  the context before the request, which remains its own message. If one cannot be
  used, the command is refused and the request is not sent (`bricks run` exits with
  2).
* No slash command is created per skill; Bricks' own commands keep their names.

An activation gives the full instructions, the skill's identity, source and revision
(a SHA-256 prefix of its `SKILL.md`), its directory, and a bounded list of its files
(64 entries, 3 levels).

The activation is recorded for that agent only once its text is in the context. A
refused read (approval rules), a failed resolution, a cancelled run or an activation
that does not fit records nothing. The same revision activated again is not repeated:
the result says it is already active, and the new request is kept.

Instructions are never cut. Before accepting an activation, the engine checks it
against the context budget (the same estimate and margin as before every request)
and compacts once by the usual rules if needed. If it still does not fit, the
activation is refused, and the result says so.

## Files of a skill

Files (`scripts/`, `references/`, `assets/` or any other name) are listed on
activation, never read or run by it:

* The model reads one with `Read`, whose permissions apply.
* `Skill` with `action: "resource"` returns the checked path of a file.
* A script runs through `Bash`, with its approval, in the agent's own working
  directory: activating a skill does not change it.
* Paths are resolved from the skill's directory. Absolute paths, `..` and symbolic
  links leading outside it are refused, checked on the canonical path at the time of
  the request.

These checks cover the files the skill service hands out. They do not confine the
agent's other tool calls to the skill's directory.

## Sub-agents

* **Profile skills.** A profile's `skills: [names]` are resolved with the same
  service, trust and checks before the child is built and before any provider call.
  A missing, refused, invalid or incompatible skill, or a total over `[agents]
  skills_max_bytes` (24 000 bytes by default), refuses the launch; nothing is cut.
  The skills go whole into the child's system prompt and are reported in its result
  (`skills`, state `loaded`) and as `skill_activated` events (`origin: "profile"`,
  with the child's `agent_id`).
* **Per-agent state.** Each child — foreground, background, nested — has its own
  `Skill` tool and its own activations. Children share the session's sources and
  trust, never the parent's activations, messages or secrets.
* **Worktrees.** A child in a worktree reads its project's skills where they are (in
  the original project folder, tracked by Git or not), and works in its worktree.
* Loading a skill is a read: it takes no workspace write lock.

## Compaction, sessions

* **Compaction.** Active instructions stay available. A compaction that summarised an
  activation away puts its text back, whole and only once, right after the summary.
  The tool-result budget never removes an active activation.
* **Storage.** `skills.json` in the session's files records the identity, source,
  revision and origin of each activation, never the instructions themselves.
* **Restoration.** Each activation is checked against its source before anything of
  it is used again: same winning source, same revision, still allowed. If the file
  changed, disappeared or is no longer allowed, the activation is suspended
  (`skill_activation_failed`, `suspended: …`) and never replaced silently. Allowing the
  project's skills later re-checks suspended activations.
* An older session without `skills.json` is restored as before.
* `/clear` empties the context and the activations with it.

## Commands and events

| Interface | Command (contract) | Answer |
|---|---|---|
| `/skills [words] [page N]` | `list_skills {query, page}` | `skills` |
| `/skills reload` | `reload_skills` | `skills` (new `generation`) |
| `/skills trust` / `/skills deny` | `trust_project_skills {allow}` | `skills_trust_resolved` |
| `/skill <name> <request>` | `submit {prompt: {blocks, skills: [name]}}` | `skill_activated`, then the run |

Events (JSONL schema 5; these are additions):

* `skill_activated {agent_id?, name, source, revision, origin}`: `origin` is `user`,
  `model`, `profile` or `restored`.
* `skill_activation_failed {agent_id?, name, reason}`.
* `skills_trust_requested {project, sources}`.
* `skills_trust_resolved {project, allowed, by}`.
* `skills {listing}`: every candidate with its source, state, path, revision and
  details, plus the discovery diagnostics.

There is no event per file read during a discovery.

## SDK

```rust
use cersei::skills::{SkillAccess, SkillsService, ProjectDecision};

let access = SkillAccess::new(Arc::new(SkillsService::standard(project, home)));
access.decide_project(ProjectDecision::Granted);      // an explicit decision
let agent = Agent::builder()
    .provider(provider)
    .tools(cersei::tools::coding())
    .skills(access)        // catalog + `Skill` (registered once) + this agent's activations
    .build()?;
```

With the controller, set `EngineConfig::skills = Some(SkillsSetup::with_home(home))`.
Without a skills service an agent has no `Skill` tool and no catalog. A `SkillTool`
added by hand says that skills are not configured.

## Migration

Nothing is converted or moved automatically.

| Before | Now |
|---|---|
| `.cersei/skills/<name>/SKILL.md` | Move it to `.agents/skills/<name>/` (project) or `~/.agents/skills/<name>/` (personal). |
| `.claude/commands/<name>.md` | Create `.agents/skills/<name>/SKILL.md` with `name: <name>` and a `description`, then put the text below the frontmatter. |
| `version`, `license` (required), `platforms`, `prerequisites` | Not required. `license` stays a standard field; the others are ignored (put useful values in `metadata`). |
| `$ARGUMENTS`, `$ARGUMENTS_SUFFIX` | Not substituted. Write the instructions for a request given separately (`/skill <name> <request>`, or the `args` of `Skill`). |
| Aliases of built-in skills (`mem`, `diagnose`, `check`…) | Removed: use the names. |
| `loop` built-in | Removed: Bricks has no scheduler. |
| `cersei-skills` crate, `cersei_tools::skills`, `SkillTool::with_project_root` | The `bricks-skills` crate (`cersei::skills`), `AgentBuilder::skills`. |
