// The summary line of a finished run (the « bilan » card): outcome, duration from
// the engine's own timestamps, and the edit totals when the structured edit data
// allows them. Pure: unit-tested (tests/unit/runend-summary.test.ts).
import type { RunInfo } from '$lib/state/conversation.svelte'
import type { RunOutcome } from '$lib/ipc/engine'
import { OUTCOME_LABEL, terminationLabel, formatDuration } from './format'
import { formatToolDuration } from './timeline/toolLine'

export interface RunEndSummary {
  /** `null` while the engine has not reported the outcome. */
  outcome: RunOutcome | null
  /** French state label: Terminé, Annulé, Échec, Incomplet, or « Fin de l’exécution ». */
  label: string
  /** Milliseconds between the engine's start and finish timestamps; `null` when one is missing. */
  durationMs: number | null
  /** The duration formatted for display (« 1m 49s »), `null` when unknown. */
  duration: string | null
  /** Compact time worked; `null` when unknown. */
  worked: string | null
  /**
   * Edit operations of the run, summed per line. `null` unless every edited file
   * is text (a binary file has no line count) and the run edited at least one file.
   */
  additions: number | null
  deletions: number | null
  /** Why an incomplete run stopped, when the engine said so. */
  reason: string | null
  /** The error of a failed run, when the engine gave one. */
  error: string | null
}

/** Compact elapsed time from the engine; `null` when unknown or invalid. */
export function formatWorked(ms: number | null): string | null {
  if (ms === null || !Number.isFinite(ms) || ms < 0) return null
  return formatDuration(ms)
}

export function runEndSummary(run: RunInfo | undefined): RunEndSummary {
  const outcome = run?.outcome ?? null
  const durationMs =
    run && run.finishedAt !== undefined && run.finishedAt >= run.startedAt
      ? run.finishedAt - run.startedAt
      : null
  const files = run ? [...run.files.values()] : []
  const countable = files.length > 0 && files.every((file) => !file.binary)
  return {
    outcome,
    label: outcome ? OUTCOME_LABEL[outcome] : 'Fin de l’exécution',
    durationMs,
    duration: durationMs === null ? null : formatToolDuration(durationMs),
    worked: formatWorked(durationMs),
    additions: countable ? files.reduce((sum, file) => sum + file.added, 0) : null,
    deletions: countable ? files.reduce((sum, file) => sum + file.removed, 0) : null,
    reason: outcome === 'incomplete' && run ? terminationLabel(run.termination) : null,
    error: outcome === 'failed' && run?.error ? run.error : null,
  }
}
