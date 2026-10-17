// What the « Historique » tab shows for the project journal. An empty journal
// shows nothing under the tab; a real error stays visible with its retry.
// Pure: unit-tested (tests/unit/history-state.test.ts).

export type HistoryView = 'no_project' | 'failed' | 'loading' | 'empty' | 'entries'

export interface HistoryFacts {
  hasProject: boolean
  loaded: boolean
  /** Last load error, if any. */
  error: string | null
  /** Records held by the journal. */
  count: number
}

export function historyView(facts: HistoryFacts): { view: HistoryView; inlineError: string | null } {
  if (!facts.hasProject) return { view: 'no_project', inlineError: null }
  if (facts.error !== null && facts.count === 0) return { view: 'failed', inlineError: null }
  if (!facts.loaded) return { view: 'loading', inlineError: null }
  if (facts.count === 0) return { view: 'empty', inlineError: null }
  // Records are shown; a failed « Charger plus » is reported above them.
  return { view: 'entries', inlineError: facts.error }
}
