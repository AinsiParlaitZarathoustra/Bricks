# Skills

Bricks reads skills in the [Agent Skills](https://agentskills.io/specification)
format. A skill is a directory holding a `SKILL.md` — YAML frontmatter, then
Markdown instructions — and, optionally, other files. Loading a skill gives the
agent instructions and the list of its files. It runs no command, starts no
agent and grants no permission. Every action still goes through the agent's
tools and the session's permission policy (see `docs/cli.md#permissions`).

The service is the `bricks-skills` crate. The `Skill` tool, the session agent,
sub-agents, the CLI, the interface and the SDK all use it.

Bricks ships one skill, `create-skills`, which explains how to turn a procedure
into a skill and install it. Every other skill is one the person installed.

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
  **incompatible**. Installation refuses such a folder, and the reason names the
  directive. Bricks never removes a directive silently: convert the skill first
  (remove the directive, keeping only what Bricks supports). A skill from a source
  you configured yourself that is incompatible can be inspected (`/skills`) but is
  not activated. `disable-model-invocation: false` and `user-invocable: true` are
  their defaults and change nothing.
* Nothing is substituted or executed: `$ARGUMENTS` and `` !`command` `` stay text
  (a note says so). A request is given separately (see *Activation*).

Reading the standard documents of a folder you configure explicitly is not full
compatibility with Claude Code skills.

## Where skills come from

Bricks reads two places by default:

| # | Source | Scope |
|---|---|---|
| 1 | `<bricks_home>/skills/<name>/SKILL.md`, the store | installed by the person |
| 2 | built into Bricks: `create-skills` | built-in |

`<bricks_home>` is `BRICKS_HOME` when it is set, otherwise `~/.bricks`.

* **Bricks does not read `~/.claude/skills`, `~/.agents/skills`,
  `<project>/.claude/skills` or `<project>/.agents/skills` by default**, on the
  desktop or in the CLI. Those folders are never read, copied, modified or deleted
  by Bricks. A skill you want in Bricks is installed in the store (see *Managing
  skills*).
* An embedder (SDK) can add sources explicitly: `SkillRoot::explicit(id, path,
  trusted)` in `SkillsSetup::extra_roots`, after the default ones. Nothing is added
  because a project file asked for it. `bricks_skills::standard_roots` is kept as an
  opt-in helper for an embedder that wants the former folders; Bricks does not call
  it.
* Only these folders are read, one level deep. Bricks does not walk parent folders,
  sub-projects or the home folder.
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

The built-in skill `create-skills` is a standard document compiled into the binary
and read by the same parser. Its instructions are not in the system prompt: they
are loaded when the skill is activated. A skill you install may take any name,
including the names of the former built-in skills (`commit`, `debug`, `remember`,
`simplify`, `stuck`, `verify`), which Bricks no longer ships. There is no `loop`
skill: Bricks schedules nothing.

Sources are read when a session opens and again when the store changes (see
*Live updates*), never on every turn. A change does not alter instructions that
are already active; the withdrawal rules below decide what happens to them.

## Managing skills

Skills a person installs live in the store, `<bricks_home>/skills/<name>/`. Each one
is a portable Agent Skills folder: `SKILL.md` and, optionally, other files. Any
compatible client can read it. A hidden file, `.bricks-install.json`, holds the
installation's identifier; the skill does not need it to be read.

### Desktop: Settings › Skills

The panel keeps its list and search. Three actions:

* **Ajouter** (add): a form with *Nom*, *Description* and *Instructions*. Errors
  appear under the field concerned, and the text you typed is kept after an error.
* **Importer** (import): choose a folder. Bricks copies the whole folder — every
  file, at any depth, licences included — into a staging area of the store,
  validates it under the final folder name, and publishes it in one step. The folder
  you chose is left intact. Symbolic links and special files are refused. No script
  is run. `.git` and `.DS_Store` are not copied, and an `.bricks-install.json` in the
  folder is ignored.
* **Supprimer** (remove): only for skills installed in the store. `create-skills`
  and skills from sources you configured cannot be removed. A removal asks for
  confirmation in the list.

Changes apply without a restart (see *Live updates*).

### `Skill` tool: `validate` and `install`

The model can check and install a skill folder with the `Skill` tool:

* `action: "validate"`, `path`: checks the folder with the same rules and writes
  nothing.
* `action: "install"`, `path`: copies the folder into the store, validates the copy
  again and publishes it. It is the only action that writes, and it needs the write
  permission (asked like any other write). Other actions only read. The folder is
  left as it was; nothing in it is run.

After an installation the skill is in the catalog at once: activate it with
`action: "activate"`.

### Removing and reinstalling

Removing a skill takes it out of the store. An activation of it is withdrawn at the
next request (see *Withdrawal*). Installing a skill again gives it a new identity:
an activation of the old installation never comes back by itself.

## Live updates

* Each change Bricks makes to the store increments a counter in the process. The
  service also compares the store folder's modification date. A change in either is
  seen at the next read, and the sources are read again.
* The settings, the composer's skill menu, and open conversations (their sub-agents
  included) see the change at their **next request**. A request that was already sent
  is not changed.
* No restart is needed.

## Trust of project folders

No project folder is read by default, so no project skill reaches the model unless
you configure one as a source explicitly (an embedder's `extra_roots`, or a source
you have added yourself).

For a configured source that is a project folder, the trust decision is taken for
that project and that session:

* **Interactive**: Bricks reports what it found (`skills_trust_requested`, asked once
  per session). `/skills` lists those skills for you to inspect. Then `/skills trust`
  allows them and `/skills deny` refuses them.
* **Headless**: `bricks run --trust-project-skills …` allows them for that session
  only. Without it, a notice says they are not used. Nothing waits for an answer.
  The flag concerns only explicitly configured sources, and grants no other
  permission.
* **SDK**: `SkillsSetup::trust_project` when the session is opened,
  `Controller::trust_project_skills(bool)`, or `SkillAccess::decide_project(…)` for an
  agent built directly.

Until a decision is taken, no name, description or instruction of such a skill reaches
the model. The model cannot approve them (the `Skill` tool has no such action), and
neither can a file of the repository, `bricks.toml` or a skill.

A refusal leaves the session usable with the other sources, and Bricks does not ask
again in that session. The decision is not stored as a blanket permission. A resumed
session starts undecided, as does a change of project.

The store and the built-in skill are usable without asking. That is the local policy:
an embedder can turn personal skills off (`SkillsSetup::personal = false`).

Trust lets the model *read* a project's procedures. It approves nothing they
describe: `Write`, `Bash`, MCP, network and delegation keep their own approvals.

## Activation

* **By the model.** The system prompt carries a catalog of the skills it may activate
  (name and description only, escaped data in one `<available_skills>` block), with a
  short guidance on the `Skill` tool. The catalog is bounded (100 entries, 16 KiB) and
  says how many skills it leaves out; `Skill` with `action: "list"` and a `query` finds
  them. The model activates one with `Skill` (`action: "activate"`, `skill`, and
  optionally `args`: the request, returned separately, never substituted into the
  text). `skill: "list"` without an `action` still lists, so a skill really named
  `list` is activated with `action: "activate"`.
* **By a person.** `/skill <name> <request>` in the interface, or `bricks run --skill
  <name> …` (repeatable). Each skill is checked before anything is sent and placed in
  the context before the request, which remains its own message. If one cannot be
  used, the command is refused and the request is not sent (`bricks run` exits with
  2).
* No slash command is created per skill; Bricks' own commands keep their names.

An activation gives the full instructions, the skill's identity, its source, its
revision (a SHA-256 prefix of its `SKILL.md`), its directory, and a bounded list of
its files (64 entries, 3 levels). A store skill's source is written `store:<id>`,
where `<id>` is that installation's identifier.

The activation is a block of data, not markup, and is marked by its identifier:
`<skill_activation id="…">`. Its name, source, revision, directory, file names,
`allowed-tools`, compatibility and body are XML-escaped (`<`, `>`, `&` and quotes become
entities, and the reserved cache marker is neutralised), so a body cannot close the
block or pass for a system instruction. The model therefore reads those characters as
entities inside the instructions, for example `&lt;` for `<`.

The activation is recorded for that agent only once its text is in the context. A
read the permission policy refused, a failed resolution, a cancelled run or an activation
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

## Withdrawal

Before each request, the engine checks every activation against its source: the
installation's identity, the source and the revision. An activation whose skill was
**removed, reinstalled or changed** is withdrawn:

* Its instructions in the active history are replaced by a short note. Tool calls and
  their results stay paired. The raw history and the user's messages are not touched.
* A sub-agent's preloaded copy of the skill leaves its system prompt.
* A compaction summary written from the withdrawn text is set aside. The active
  history is rebuilt from the pre-compaction snapshots, cleaned, and compacted again
  when it does not fit.
* A reinstalled skill is a new identity and is not reactivated silently. Activate it
  again to use it.

The withdrawal is reported as a `skill_activation_failed` event.

Limits of this check:

* A summary is judged tainted from the pre-compaction snapshots, transitively: a
  summary written from a tainted summary is tainted too.
* Snapshots are saved with the memory when a memory and a session identifier exist.
  Without them, they live in the process only.
* A session whose snapshots are missing cannot be checked this way. Its summary stays
  as it is; only the active history is cleaned.

## Sub-agents

* **Profile skills.** A profile's `skills: [names]` are resolved with the same
  service, trust and checks before the child is built and before any provider call.
  A missing, refused, invalid or incompatible skill, or a total over `[agents]
  skills_max_bytes` (24 000 bytes by default), refuses the launch; nothing is cut.
  The skills go whole into the child's system prompt and are reported in its result
  (`skills`, state `loaded`) and as `skill_activated` events (`origin: "profile"`,
  with the child's `agent_id`). A skill removed or changed later is withdrawn from
  that system prompt at the next request.
* **Per-agent state.** Each child — foreground, background, nested — has its own
  `Skill` tool and its own activations. Children share the session's sources and
  trust, never the parent's activations, messages or secrets.
* **Worktrees.** A child in a worktree reads the same store as its parent and works
  in its worktree.
* Loading a skill is a read: it takes no workspace write lock.

## Compaction and sessions

* **Compaction.** Active instructions stay available. A compaction that summarised an
  activation away puts its text back, whole and only once, right after the summary.
  The tool-result budget never removes an active activation. Withdrawal applies to
  these summaries as described above.
* **Storage.** `skills.json` in the session's files records the identity, source,
  revision and origin of each activation, never the instructions themselves.
* **Restoration.** Each activation is checked against its source before anything of
  it is used again: same installation, same source, same revision. If the skill was
  removed or changed, the activation is withdrawn, as above.
* An older session without `skills.json` is restored as before.
* `/clear` empties the context and the activations with it.

## Commands and events

| Interface | Command (contract) | Answer |
|---|---|---|
| `/skills [words] [page N]` | `list_skills {query, page}` | `skills` |
| `/skills reload` | `reload_skills` | `skills` (new `generation`) |
| `/skills trust` / `/skills deny` | `trust_project_skills {allow}` | `skills_trust_resolved` |
| `/skill <name> <request>` | `submit {prompt: {blocks, skills: [name]}}` | `skill_activated`, then the run |

Events (added in JSONL schema 5 and unchanged in schemas 6 and 7):

* `skill_activated {agent_id?, name, source, revision, origin}`: `origin` is `user`,
  `model`, `profile` or `restored`.
* `skill_activation_failed {agent_id?, name, reason}`: a refused activation, or a
  withdrawal.
* `skills_trust_requested {project, sources}`.
* `skills_trust_resolved {project, allowed, by}`.
* `skills {listing}`: every candidate with its source, state, path, revision and
  details, plus the discovery diagnostics.

There is no event per file read during a discovery.

## SDK

```rust
use bricks::skills::{SkillAccess, SkillsService, ProjectDecision};

let access = SkillAccess::new(Arc::new(SkillsService::standard(project, Some(store))));
access.decide_project(ProjectDecision::Granted);      // an explicit decision, for configured project sources
let agent = Agent::builder()
    .provider(provider)
    .tools(bricks::tools::coding())
    .skills(access)        // catalog + `Skill` (registered once) + this agent's activations
    .build()?;
```

`SkillsService::standard(project, store)` reads the store (when given) and the built-in
skill, and no folder of the project or of the home directory. Other sources are added
with `SkillsService::with_roots`, or through `SkillsSetup::extra_roots` on the
controller. With the controller, set `EngineConfig::skills =
Some(SkillsSetup::with_store(Some(store_dir)))`; `None` for the store means only the
built-in skill.

Without a skills service an agent has no `Skill` tool and no catalog. A `SkillTool`
added by hand says that skills are not configured.

## Migration

Nothing is converted, moved or deleted automatically, and the folders of other
tools are never touched. The old built-in skills (`commit`, `debug`, `remember`,
`simplify`, `stuck`, `verify`) are removed. To use a skill again, import its folder in
*Settings › Skills* or ask Bricks to install it with the `Skill` tool. See
`docs/content/docs/migration.mdx`.
