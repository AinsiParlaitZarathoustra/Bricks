# Sprint 14.1 — additional rules (read AGENT_RULES.md first; it still applies)

Spec: `docs/sprint-14-1-ui.md` (repo root). Goal: migrate the TUI's `/` commands and inspectors into the
desktop, simplify composer placeholder and settings, then (later wave) remove the TUI.

## Shared pieces (orchestrator-owned: read, don't edit — report needs)
- Contract: `src-tauri/src/contract.rs` section "Sprint 14.1" (`SlashCommandInfo`, `SlashAction`,
  `SlashPresent`, `SessionToolInfo`, `McpServerInfo`, `SessionConfigInfo`, `ConfigEntry`); command list in
  `src-tauri/src/lib.rs`; bindings `src/lib/ipc/bindings/`.
- IPC client `src/lib/ipc/client.ts`: `slashCommands()`, `slashParse(line)`, `sessionTools(id)`,
  `sessionMcp(id)`, `sessionConfig(id)`, `sessionLocate(id)`, `appQuit()` (+ all Sprint 14 functions).
- Reducer `src/lib/state/conversation.svelte.ts` — new per-session fields: `turnUsages`, `usageTotalFull`,
  `runUsages`, `jobs` (JobInfo), `searchResult`, `lastClear`, `agentProfiles`, `skillsListing`,
  `memory` (lastRecall, maintenanceRunning, lastMaintenance), `lastCompaction`, `lastSkill`,
  `lastAgentControl`, `lastRejection`, plus Sprint 14 fields (`context`, `agents`, `changesets`, ...).
  Engine JSON types: `src/lib/ipc/engine.ts`.
- `src/lib/state/app.svelte.ts`: new `app.resumeSession(id)` (locates the project, reopens through the
  desktop registry; returns `null` or a French refusal). `app.command(sessionId, Command)` returns `null`
  or the refusal.
- `src/lib/state/ui.svelte.ts`: `ui.open(view, args)` / `ui.close()` for inspector views (rendered in a
  Dialog by `features/commands/InspectorHost.svelte`, orchestrator), `ui.openPicker('model'|'effort')`.
  View components receive `{ args }` and read `app.current` themselves.
- Engine answers are asynchronous events: after sending a command, show a pending state until the
  matching event lands in the reducer field; a refusal (`app.command` result or
  `conversation.lastRejection`) must be shown in that view. Never fake a result.
- All UI text French. Keep command names/aliases exactly as the TUI registry (`crates/bricks-tui/src/commands.rs`).
- Reuse existing components (AgentList/AgentInspector in features/agents, ContextRing, ChangesPanel,
  navigation search...) instead of duplicating; import them, don't edit other owners' files.
