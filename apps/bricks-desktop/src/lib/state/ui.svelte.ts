// Inspector views opened by `/` commands or menus. One view at a time,
// rendered by `features/commands/InspectorHost.svelte`. Owner: orchestrator.

export type InspectorView =
  | 'help'
  | 'search'
  | 'memory'
  | 'context'
  | 'cost'
  | 'session'
  | 'sessions'
  | 'tools'
  | 'mcp'
  | 'config'
  | 'agents'
  | 'agent_instances'
  | 'skills'
  | 'jobs'
  | 'changeset'
  | 'clear'

export interface InspectorState {
  view: InspectorView
  /** View options: `details` (context, cost), `query`, `changesetId`, `agentId`... */
  args: Record<string, string | boolean | undefined>
}

/** Pickers of the composer that a command can open. */
export type ComposerPicker = 'model' | 'effort' | null

class UiStore {
  inspector = $state<InspectorState | null>(null)
  /** Set by `/model`, `/effort`; the composer opens the matching select and resets it. */
  picker = $state<ComposerPicker>(null)
  /** The settings dialog (opened from the left panel footer). */
  settingsOpen = $state(false)

  open(view: InspectorView, args: InspectorState['args'] = {}) {
    this.inspector = { view, args }
  }

  close() {
    this.inspector = null
  }

  openPicker(p: Exclude<ComposerPicker, null>) {
    this.picker = p
  }
}

export const ui = new UiStore()
