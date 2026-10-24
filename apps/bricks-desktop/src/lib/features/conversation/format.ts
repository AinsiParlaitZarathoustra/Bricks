// Display helpers for the conversation timeline. Pure functions only.
import type { ChangeKind, Decision, DecidedBy, RunOutcome, Termination } from '$lib/ipc/engine'

const numberFormat = new Intl.NumberFormat('fr-FR', { maximumFractionDigits: 1 })
const integerFormat = new Intl.NumberFormat('fr-FR')

const SHELL_TOOLS = new Set(['Bash', 'bash', 'Shell', 'PowerShell'])
// `Edit` may carry `edits` (several replacements in one call): its summary is still the path.
// `MultiEdit` and `NotebookEdit` are historical names of earlier calls: kept as
// recorded so old conversations still show their path, never offered as tools.
const PATH_TOOLS = new Set(['Read', 'Write', 'Edit', 'MultiEdit', 'NotebookEdit'])
const SEARCH_TOOLS = new Set(['Grep', 'Glob'])

export const OUTCOME_LABEL: Record<RunOutcome, string> = {
  succeeded: 'Terminé',
  incomplete: 'Incomplet',
  failed: 'Échec',
  cancelled: 'Annulé',
}

export const DECISION_LABEL: Record<Decision, string> = {
  allow: 'Autorisé',
  allow_for_session: 'Autorisé pour la session',
  deny: 'Refusé',
}

export const DECIDED_BY_LABEL: Record<DecidedBy, string> = {
  user: 'par vous',
  session: 'selon l’autorisation de la session',
  non_interactive: 'mode non interactif',
  cancelled: 'annulée',
}

export const CHANGE_KIND_LABEL: Record<ChangeKind, string> = {
  create: 'Créé',
  modify: 'Modifié',
  delete: 'Supprimé',
}

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

export function oneLine(text: string): string {
  return text.replace(/\s+/g, ' ').trim()
}

export function truncate(text: string, max: number): string {
  return text.length > max ? `${text.slice(0, Math.max(0, max - 1))}…` : text
}

export function safeJson(value: unknown): string {
  try {
    return JSON.stringify(value) ?? String(value)
  } catch {
    return '[valeur non affichable]'
  }
}

/** Pretty JSON for the detail views (never throws). */
export function prettyJson(value: unknown): string {
  try {
    return JSON.stringify(value, null, 2) ?? String(value)
  } catch {
    return '[valeur non affichable]'
  }
}

function firstString(obj: Record<string, unknown>, keys: string[]): string | null {
  for (const key of keys) {
    const v = obj[key]
    if (typeof v === 'string' && v.length > 0) return v
  }
  return null
}

/** One-line summary of a tool input: the command, the path, or the first keys. */
export function summarizeInput(name: string, input: unknown): string {
  if (input === undefined || input === null) return ''
  if (typeof input === 'string') return truncate(oneLine(input), 160)
  if (!isRecord(input)) return truncate(oneLine(safeJson(input)), 160)
  if (SHELL_TOOLS.has(name) && typeof input.command === 'string') {
    return truncate(oneLine(input.command), 160)
  }
  if (PATH_TOOLS.has(name)) {
    const path = firstString(input, ['file_path', 'path', 'notebook_path'])
    if (path) return truncate(path, 160)
  }
  if (SEARCH_TOOLS.has(name) && typeof input.pattern === 'string') {
    const path = firstString(input, ['path'])
    return truncate(oneLine(input.pattern) + (path ? ` dans ${path}` : ''), 160)
  }
  const parts = Object.entries(input)
    .slice(0, 3)
    .map(([key, value]) => `${key}: ${typeof value === 'string' ? oneLine(value) : safeJson(value)}`)
  return truncate(parts.join(' · '), 160)
}

export function formatDuration(ms: number | undefined): string {
  if (ms === undefined || !Number.isFinite(ms) || ms < 0) return '—'
  if (ms < 1000) return `${Math.round(ms)}ms`
  const total = Math.floor(ms / 1000)
  const hours = Math.floor(total / 3600)
  const minutes = Math.floor((total % 3600) / 60)
  const seconds = total % 60
  return [hours && `${hours}h`, minutes && `${minutes}m`, (seconds || total === 0) && `${seconds}s`]
    .filter(Boolean).join(' ')
}

export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return ''
  const units = ['o', 'Ko', 'Mo', 'Go']
  let value = bytes
  let unit = 0
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024
    unit += 1
  }
  const digits = unit === 0 ? integerFormat : numberFormat
  return `${digits.format(value)} ${units[unit]}`
}

export function basename(path: string): string {
  const parts = path.split(/[\\/]/)
  return parts[parts.length - 1] || path
}

function numberField(value: Termination, key: string): number | null {
  const v = (value as Record<string, unknown>)[key]
  return typeof v === 'number' ? v : null
}

/** Human reason for a run that did not complete normally, or null. */
export function terminationLabel(termination: Termination | undefined): string | null {
  if (!termination) return null
  switch (termination.kind) {
    case 'completed':
      return null
    case 'output_truncated': {
      const n = numberField(termination, 'continuations')
      return n === null ? 'sortie tronquée' : `sortie tronquée après ${n} continuation${n > 1 ? 's' : ''}`
    }
    case 'no_progress': {
      const n = numberField(termination, 'repeats')
      return n === null ? 'aucun progrès détecté' : `aucun progrès détecté (${n} répétitions)`
    }
    default:
      // A kind of an earlier version: interrupted, never a success.
      return 'fin interrompue (ancienne session)'
  }
}

export interface TestCounts {
  framework: string
  passed: number
  failed: number
}

function count(text: string, word: string): number | null {
  const m = text.match(new RegExp(`(\\d+) ${word}\\b`))
  return m ? Number(m[1]) : null
}

/**
 * Reads pass/fail totals from a test runner's own report. Returns null when
 * the output is not a recognised report, so no count is shown.
 */
export function parseTestCounts(output: string): TestCounts | null {
  const cargo = [...output.matchAll(/test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed/g)]
  if (cargo.length > 0) {
    return {
      framework: 'cargo test',
      passed: cargo.reduce((sum, m) => sum + Number(m[1]), 0),
      failed: cargo.reduce((sum, m) => sum + Number(m[2]), 0),
    }
  }
  const pytest = output.match(/^=+ (.*\b(?:passed|failed|errors?)\b.*) in [\d.]+s.*=+\s*$/m)
  if (pytest) {
    const passed = count(pytest[1], 'passed')
    const failed = count(pytest[1], 'failed')
    if (passed !== null || failed !== null) {
      return { framework: 'pytest', passed: passed ?? 0, failed: failed ?? 0 }
    }
  }
  const vitest = output.match(/^\s*Tests\s+(.+?)\s*$/m)
  if (vitest) {
    const passed = count(vitest[1], 'passed')
    const failed = count(vitest[1], 'failed')
    if (passed !== null || failed !== null) {
      return { framework: 'vitest', passed: passed ?? 0, failed: failed ?? 0 }
    }
  }
  const mocha = output.match(/^\s*(\d+) passing\b/m)
  if (mocha) {
    const failing = output.match(/^\s*(\d+) failing\b/m)
    return { framework: 'mocha', passed: Number(mocha[1]), failed: failing ? Number(failing[1]) : 0 }
  }
  return null
}

/** French count: the singular is used for 0 and 1. */
export function plural(n: number, singular: string, pluralForm = `${singular}s`): string {
  return `${n} ${n > 1 ? pluralForm : singular}`
}
