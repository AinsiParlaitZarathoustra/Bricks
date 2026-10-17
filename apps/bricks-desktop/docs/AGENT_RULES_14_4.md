# Sprint 14.4 — rules (they OVERRIDE the frontend-wave and effort sections of earlier rule files)

Spec: `docs/sprint-14-4-ui.md` is not a file — the spec is quoted in your brief.

- **Haiku agents never edit `.svelte` components, CSS, layout, typography or visible texts.** Opus (the
  orchestrator) owns every visual change. Haiku may edit only the TypeScript/Rust files named in its brief
  (`.svelte.ts` only for reactive logic, no templates/styles). Report visual defects instead of fixing them.
- Effort/model are verified by the orchestrator from launch metadata — do not self-declare them.
- One owner per shared file at a time (named in each brief). Generated bindings, `contract.rs`, `lib.rs`,
  `client.ts`, `engine.ts`, Cargo/npm manifests: orchestrator only.
- Never touch `crates/cersei-agent/src/system_prompt.rs`, `bricks.toml`, `docs/sprint-14*.md`,
  `~/.bricks/**` user data (sessions, provider keys). No cleanup of user files.
- Stop every process you start (servers, `yes`, watchers) before reporting. No global process kills.
- No mock data in the product. Report real check results only.

## Contracts fixed for this sprint
- `session_command(sessionId, command, display: string | null)` — `display` = typed command label of a
  submit (e.g. `/skill debug`), never sent to the engine.
- `DesktopEvent::RunLabeled { session_id, label: RunLabel }`, `RunLabel { run_id, label, prompt, at }`,
  `OpenedSession.run_labels: RunLabel[]` (oldest first).
