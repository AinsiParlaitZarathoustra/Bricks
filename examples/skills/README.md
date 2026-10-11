# Example skills

Two standard [Agent Skills](https://agentskills.io/specification) for Bricks:

- `hello-skill/` — the minimum: a `SKILL.md` with `name`, `description` and instructions.
- `release-notes/` — optional fields and a local resource (`references/format.md`),
  listed when the skill is activated and read with `Read` only when needed.

To try one, copy its folder into `~/.agents/skills/` (personal) or into a project's
`.agents/skills/` (project skills need your approval in the session: `/skills trust`, or
`--trust-project-skills` with `bricks run`). See `docs/skills.md`.
