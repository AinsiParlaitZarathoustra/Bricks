# Skills

A skill is a folder with a `SKILL.md` file and, if you want, other files. `SKILL.md` has a short header (YAML) and then instructions in Markdown. Bricks reads the [Agent Skills](https://agentskills.io/specification) format.

Loading a skill gives the agent the instructions and the list of the skill's files. That is all. A skill runs no command, starts no agent and grants no permission. Every action still goes through the agent's tools and the session's permission policy.

The code is in the `bricks-skills` crate. The `Skill` tool, the agents, the CLI, the desktop app and the SDK all use it.

Bricks ships one skill, `create-skills`. It explains how to turn a procedure into a skill and install it. Every other skill is one you installed.

## Format

```markdown
---
name: release-notes
description: Draft release notes for the changes since the last tag. Use when asked for a changelog.
license: MPL-2.0
compatibility: Needs a Git repository.
metadata:
  team: docs
allowed-tools: Bash(git:*) Read
---
# Release notes

1. Find the last tag.
2. List the commits since then.
```

* `name` (required): 1 to 64 characters. Lowercase letters, digits and `-`. It cannot start or end with `-` or contain `--`. It must match the folder name.
* `description` (required): 1 to 1024 characters. Say what the skill does and when to use it. The model sees only this text before it activates the skill.
* `license`, `compatibility` (at most 500 characters) and `metadata` (text values) are optional and kept as written.
* `allowed-tools` is optional. Bricks shows it but does not apply it. It grants and restricts nothing.
* A `SKILL.md` can be at most 100 KiB, and its header at most 32 KiB. Duplicate keys are refused.

### Fields Bricks does not support

* Extra informational fields (`version`, `tags`, `argument-hint` and similar) are ignored with a note.
* Claude Code settings that change how a skill runs (`context`, `agent`, `model`, `effort`, `hooks`, `paths`, `shell`, `disable-model-invocation: true`, `user-invocable: false`) make the skill **incompatible**. Installing it is refused and the message names the setting. Remove the setting first.
* Nothing is substituted or run. `$ARGUMENTS` and `` !`command` `` stay plain text.

## Where skills come from

Bricks reads two places:

| Source | Meaning |
|---|---|
| `<bricks_home>/skills/<name>/SKILL.md` | Skills you installed (the store). |
| Built in | `create-skills`. |

`<bricks_home>` is `BRICKS_HOME` if it is set, otherwise `~/.bricks`.

* Bricks does not read `~/.claude/skills`, `~/.agents/skills` or the `.claude/skills` and `.agents/skills` folders of a project. It never changes them. To use one of those skills, install it in the store.
* An app built with the SDK can add other folders on purpose (`SkillRoot::explicit` in `SkillsSetup::extra_roots`). A project file cannot add one.
* Folders are read one level deep. Symbolic links are not followed.
* If two skills have the same name, `/skills` shows the winner and the ones it hides. An invalid winner makes its name fail. There is no silent fallback to a lower skill with the same name.
* Skills are read when a session opens and again when the store changes. A change does not alter instructions that are already active.

## Managing skills

Your skills live in `<bricks_home>/skills/<name>/`. A hidden file, `.bricks-install.json`, records the installation. Other clients can still read the folder.

### Desktop: Settings, Skills

* **Add**: a form with a name, a description and the instructions. Errors show under the field, and your text is kept.
* **Import**: choose a folder. Bricks copies all of it into a staging area, checks it, and then publishes it. Your folder is not changed. Symbolic links are refused. No script is run. `.git` and `.DS_Store` are not copied.
* **Remove**: only for skills installed in the store. `create-skills` cannot be removed. You confirm in the list.

Changes apply without a restart.

### The `Skill` tool

The model can check and install a skill folder:

* `action: "validate"` with `path`: checks the folder and writes nothing.
* `action: "install"` with `path`: copies the folder into the store, checks the copy and publishes it. This is the only action that writes, so it asks for write permission.

After an install, activate the skill with `action: "activate"`.

Removing a skill takes it out of the store. Installing it again makes a new installation, and an old activation does not come back by itself.

## Project skills and trust

No project folder is read by default. A project skill reaches the model only if someone configured that folder as a source. For such a source, the trust decision is per project and per session:

* **Interactive**: Bricks asks once. `/skills` lists what it found. `/skills trust` allows them and `/skills deny` refuses them.
* **Headless**: `bricks run --trust-project-skills` allows them for that run. Without it, they are not used and nothing waits for an answer.
* **SDK**: `SkillsSetup::trust_project`, `Controller::trust_project_skills(bool)`, or `SkillAccess::decide_project(...)`.

Until a decision is made, nothing from those skills reaches the model. The model cannot approve them, and neither can a repository file. Trust lets the model read the procedures. It approves nothing they describe: `Write`, `Bash`, MCP and the network keep their own approvals.

## Activation

* **By the model.** The system prompt has a short catalog (name and description) of the skills it can use. The catalog holds at most 100 entries and 16 KiB, and says how many are left out. `Skill` with `action: "list"` and a `query` finds the others. The model activates one with `action: "activate"`, `skill`, and optionally `args` (the request, passed separately).
* **By you.** `/skill <name> <request>` in the interface, or `bricks run --skill <name>` (repeatable). Each skill is checked first. If one cannot be used, the command is refused and nothing is sent (`bricks run` exits with 2).
* No slash command is created for each skill.

An activation gives the full instructions, the skill's source, its revision (a SHA-256 prefix of `SKILL.md`), its folder, and a list of its files (64 entries, 3 levels deep). The text is escaped, so a skill body cannot pass for a system instruction.

Instructions are never cut. If an activation does not fit in the context, Bricks compacts once. If it still does not fit, the activation is refused. Activating the same revision again does nothing, and the answer says it is already active.

## Files of a skill

Files (`scripts/`, `references/`, `assets/`, or any name) are listed on activation. Activation does not read or run them.

* The model reads one with `Read`, under the usual permissions.
* `Skill` with `action: "resource"` returns the checked path of a file.
* A script runs through `Bash`, with approval, in the agent's own working directory.
* Paths start at the skill's folder. Absolute paths, `..` and links that lead outside are refused.

## Withdrawal

Before each request, Bricks checks every activation against its source. If the skill was removed, reinstalled or changed, the activation is withdrawn:

* Its instructions in the active history become a short note.
* A sub-agent's preloaded copy leaves its system prompt.
* A compaction summary written from the old text is set aside and rebuilt.

A `skill_activation_failed` event reports it. Activate the skill again to use it.

## Sub-agents

* A profile's `skills: [names]` are loaded whole into the sub-agent's system prompt (see `docs/agents.md`). A missing, invalid or incompatible skill, or a total over `[agents] skills_max_bytes` (24000 by default), refuses the launch.
* Each sub-agent has its own `Skill` tool and its own activations. It shares the session's sources and trust, not the parent's activations.

## Compaction and sessions

* A compaction that summarized an activation away puts its text back, once, right after the summary.
* `skills.json` in the session's files records the name, source, revision and origin of each activation. It never stores the instructions.
* When a session is restored, each activation is checked against its source. A removed or changed skill is withdrawn.
* `/clear` empties the context and the activations.

## Commands

| Interface | Command | Answer |
|---|---|---|
| `/skills [words] [page N]` | `list_skills` | `skills` |
| `/skills reload` | `reload_skills` | `skills` |
| `/skills trust`, `/skills deny` | `trust_project_skills` | `skills_trust_resolved` |
| `/skill <name> <request>` | `submit` with `skills: [name]` | `skill_activated`, then the run |

Events: `skill_activated` (origin `user`, `model`, `profile` or `restored`), `skill_activation_failed`, `skills_trust_requested`, `skills_trust_resolved`, and `skills` (the list with sources, states and problems).

## SDK

```rust
use bricks::skills::{SkillAccess, SkillsService, ProjectDecision};

let access = SkillAccess::new(Arc::new(SkillsService::standard(project, Some(store))));
access.decide_project(ProjectDecision::Granted);
let agent = Agent::builder()
    .provider(provider)
    .tools(bricks::tools::coding())
    .skills(access)
    .build()?;
```

`SkillsService::standard(project, store)` reads the store (if given) and the built-in skill. Add other sources with `SkillsService::with_roots` or `SkillsSetup::extra_roots`. With the controller, set `EngineConfig::skills = Some(SkillsSetup::with_store(Some(store_dir)))`. A `None` store means only the built-in skill.

Without a skills service, an agent has no `Skill` tool and no catalog.

## Older skills

Nothing is converted or deleted for you. The old built-in skills (`commit`, `debug`, `remember`, `simplify`, `stuck`, `verify`) are gone. To use one again, import its folder in Settings, Skills, or ask Bricks to install it with the `Skill` tool. See `docs/content/docs/migration.mdx`.
