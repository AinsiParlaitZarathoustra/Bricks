// Labels and number formats shared by the changes views.
import { t, dateTimeFormat } from '$lib/i18n/i18n.svelte';


const DATE_TIME = dateTimeFormat({
  day: 'numeric',
  month: 'short',
  hour: '2-digit',
  minute: '2-digit',
})

/** What the edit counts are: sums of edit operations, not a Git diff. */
export const editSumLabel = (): string => t('changes.sumOfEditOperationsNotA')

export function formatDateTime(ms: number): string {
  return DATE_TIME.format(new Date(ms))
}

/** Label of a change-journal `kind` (`create` | `modify` | `delete`). */
export function kindLabel(kind: string): string {
  switch (kind) {
    case 'create':
      return t('common.created')
    case 'modify':
      return t('common.modified')
    case 'delete':
      return t('common.deleted')
    default:
      return kind
  }
}

/** Label of a porcelain status letter from `git status`. */
export function gitCodeLabel(code: string): string {
  switch (code) {
    case 'M':
      return t('common.modified')
    case 'A':
      return t('common.added')
    case 'D':
      return t('common.deleted')
    case 'R':
      return t('common.renamed')
    case 'C':
      return t('common.copied')
    case 'T':
      return t('changes.typeChanged')
    case 'U':
      return t('common.conflict')
    case '?':
      return t('changes.untracked2')
    default:
      return code
  }
}

export function baseName(path: string): string {
  const parts = path.split('/')
  return parts[parts.length - 1] || path
}

export function dirName(path: string): string {
  const index = path.lastIndexOf('/')
  return index > 0 ? path.slice(0, index) : ''
}

/** Short form of a hash or an opaque id, for display only. */
export function shortId(id: string): string {
  return id.length > 8 ? id.slice(0, 8) : id
}

