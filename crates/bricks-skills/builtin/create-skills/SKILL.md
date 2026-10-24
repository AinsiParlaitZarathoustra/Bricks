---
name: create-skills
description: Turn a procedure the user wants to reuse into a standard Agent Skill and install it in Bricks. Use when the user asks to create, write, package or install a skill.
metadata:
  origin: bricks-builtin
---
# Create a skill

A skill is a folder holding a `SKILL.md` (YAML frontmatter, then Markdown
instructions) and, when useful, other files (`scripts/`, `references/`,
`assets/`). Bricks installs a copy of the folder in its store; the copy
becomes available at once, without restarting.

1. **Understand the procedure.** Find out what the user wants to do again:
   the goal, the steps, the inputs, what a good result looks like. Ask only
   for what is really missing; reuse what the conversation already says.
2. **Choose the name and the description.**
   - `name`: lowercase letters, digits and `-` only, at most 64 characters,
     no `-` at the start or end, no `--` (for example `release-notes`).
     The folder has exactly this name.
   - `description`: what the skill does and when to use it, in one or two
     sentences (at most 1024 characters). The model decides from it alone
     whether to activate the skill.
3. **Write the folder** in a working folder outside the project unless the
   user wants it in the project, for example `<temp>/skill-drafts/<name>/`:
   - `SKILL.md`:
     ```
     ---
     name: <name>
     description: <description>
     ---
     # <Title>

     <the steps, precise and in order>
     ```
   - other files only when the instructions need them, referred to by
     relative paths from the folder (`references/format.md`,
     `scripts/check.sh`). Never absolute paths; no symbolic links.
   - Only the standard fields: `name`, `description`, and when needed
     `license`, `compatibility`, `metadata`, `allowed-tools`. Do not use
     directives such as `context`, `agent`, `model` or `hooks`: Bricks does
     not implement them and refuses the skill.
4. **Validate**: call `Skill` with `action: "validate"` and `path` set to
   the folder. Fix every error it reports in the files, then validate again
   until it passes.
5. **Install**: call `Skill` with `action: "install"` and the same `path`.
   The folder is copied and checked again; the original stays where it is.
   If the name is taken, ask the user for another name or whether to remove
   the installed skill first; never overwrite it.
6. **Check**: call `Skill` with `action: "list"` to see the new skill in
   the catalog, then activate it when the user wants to use it now.

Installing writes in Bricks' store and goes through the usual approvals.
A skill grants no permission: its instructions only describe a procedure.
