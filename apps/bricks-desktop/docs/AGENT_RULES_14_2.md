# Sprint 14.2 — additional rules (read AGENT_RULES.md and AGENT_RULES_14_1.md first; they still apply)

Spec: `docs/sprint-14-2-ui.md` does not exist as a file — the spec is quoted in your brief. Context:
`apps/bricks-desktop/docs/SPRINT14_1_WALKTHROUGH.md`. The Sprint 14.1 changes are UNCOMMITTED local
changes: never revert, reset, stash or reformat files you don't own.

## Contracts fixed by the orchestrator (read, don't change — report needs)
- `Preferences.tool_approval: ToolApproval` (`standard` | `always_ask` | `no_confirmation`), saved with
  `app.setPrefs({ tool_approval })` → `prefs_set`. Priority (one place, engine gate): forbidden → refused;
  explicit `deny` rule → refused; then always_ask → ask (session grants ignored); no_confirmation → allow;
  standard → project rules + session grants unchanged.
- Provider editing IPC (`src/lib/ipc/client.ts`): `providersConfig()` → `ProvidersFile {path, revision,
  providers: ProviderView[], error}`; `providerUpdate(id, ProviderPatch, revision)`,
  `modelUpdate(pid, mid, ModelPatch, revision)`, `providerDelete(id, revision)`,
  `modelDelete(pid, mid, revision)` → the reloaded `ProvidersFile`. Error code `conflict` when the file
  changed since `revision`. Views never contain secret values. After a successful write the frontend
  calls `app.reloadModels()`.
- `ui.settingsOpen` opens the settings dialog (the top-bar button was removed by the orchestrator).
- No simulated models, tools, costs or results. Keep `/` palette, `@` mentions, attachments, send/cancel,
  resizable panels working.
