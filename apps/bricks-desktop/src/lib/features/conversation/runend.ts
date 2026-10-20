// The summary line of a finished run (the « bilan » card): outcome, duration from
// the engine's own timestamps, and the edit totals when the structured edit data
// allows them. Pure: unit-tested (tests/unit/runend-summary.test.ts).
import type { RunInfo } from '$lib/state/conversation.svelte'
import type { RunOutcome } from '$lib/ipc/engine'
import { OUTCOME_LABEL, terminationLabel } from './format'
import { formatToolDuration } from './timeline/toolLine'

export interface RunEndSummary {
  /** `null` while the engine has not reported the outcome. */
  outcome: RunOutcome | null
  /** French state label: Terminé, Annulé, Échec, Incomplet, or « Fin de l’exécution ». */
  label: string
  /** Milliseconds between the engine's start and finish timestamps; `null` when one is missing. */
  durationMs: number | null
  /** The duration formatted for display (« 762 ms »), `null` when unknown. */
  duration: string | null
  /** The time worked, as `MM:SS` (`HH:MM:SS` from an hour); `null` when unknown. */
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

/**
 * The time a run worked, from the engine's timestamps: `MM:SS` under an hour,
 * `HH:MM:SS` from an hour (hours may pass 24). Seconds are cut, not rounded:
 * 762 ms is `00:00`. `null` when the time is unknown or negative.
 */
export function formatWorked(ms: number | null): string | null {
  if (ms === null || !Number.isFinite(ms) || ms < 0) return null
  const total = Math.floor(ms / 1000)
  const hours = Math.floor(total / 3600)
  const minutes = Math.floor((total % 3600) / 60)
  const seconds = total % 60
  const pad = (n: number) => String(n).padStart(2, '0')
  return hours > 0 ? `${pad(hours)}:${pad(minutes)}:${pad(seconds)}` : `${pad(minutes)}:${pad(seconds)}`
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
