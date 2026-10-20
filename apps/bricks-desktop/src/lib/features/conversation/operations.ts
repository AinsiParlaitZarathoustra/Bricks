// Identity keys shared by the run summaries (the « bilan ») and the changes view,
// so one edit or one command is counted once wherever it is shown. Pure.

/** Tools whose calls are commands (background jobs are followed through their call). */
const SHELL_TOOLS = new Set(['Bash', 'bash', 'Shell', 'PowerShell'])

export function isShellTool(name: string): boolean {
  return SHELL_TOOLS.has(name)
}

/** One edit operation: a tool call of a session or sub-agent, on one path. */
export function operationKey(agentId: string | null | undefined, toolCallId: string, path: string): string {
  return `${agentId ?? ''}|${toolCallId}|${path}`
}

/** One file of a run: the full path when the engine knows it, else the path as written. */
export function fileKey(runId: string, absolute: string | null | undefined, path: string): string {
  return `${runId}|${absolute ?? path}`
}

/** One command of a run: its root run, its sub-agent (none for the session agent), its call. */
export function commandKey(rootRunId: string, agentId: string | null | undefined, toolCallId: string): string {
  return `${rootRunId}|${agentId ?? ''}|${toolCallId}`
}
