# Global state (owner: orchestrator)

- `app` (`app.svelte.ts`): `info`, `models` (catalog), `prefs` + `setPrefs(patch)`, `projects`,
  `sessions` (of the current project), `currentProject`, `current` (the shown `Conversation`),
  `rightView`, `lastError`. Actions: `addProject()` (native picker), `selectProject`,
  `removeProject`, `renameProject`, `newConversation(model?, reasoning?)`, `openSession(id)`,
  `renameSession`, `deleteSession` (caller confirms first), `closeSession`, `search(query, all?)`,
  `command(sessionId, Command)` → `null` or the refusal message. `onChangeRecorded(listener)`.
- `Conversation` (`conversation.svelte.ts`), one per open session, fed even when not shown:
  `items` (timeline: `user`, `assistant` {text, thinking, streaming}, `tool`, `approval`, `edit`,
  `notice`, `run_end`), `runs` (RunInfo: startedAt/finishedAt from engine timestamps, outcome,
  finalText, `files` = summed edit operations per path, `commands` = shell evidence), `agents`
  (spawn info with requested/applied model & reasoning, state, tools, result), `changesets`,
  `approvals` (pending), `skillsTrust`, `running`/`runningRunId`, `model`/`reasoning`/
  `effectiveReasoning`, `pendingChange`, `context` + `contextStale`, `draft` (composer text and
  attachments, kept per conversation), `lastRejection`.
- Engine types: `$lib/ipc/engine`. IPC: `$lib/ipc/client`.
