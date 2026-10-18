// Labels and number formats shared by the changes views.

const DATE_TIME = new Intl.DateTimeFormat('fr-FR', {
  day: 'numeric',
  month: 'short',
  hour: '2-digit',
  minute: '2-digit',
})

/** What the edit counts are: sums of edit operations, not a Git diff. */
export const EDIT_SUM_LABEL = 'somme des opérations d’édition (pas un diff Git net)'

export function formatDateTime(ms: number): string {
  return DATE_TIME.format(new Date(ms))
}

/** Label of a change-journal `kind` (`create` | `modify` | `delete`). */
export function kindLabel(kind: string): string {
  switch (kind) {
    case 'create':
      return 'Créé'
    case 'modify':
      return 'Modifié'
    case 'delete':
      return 'Supprimé'
    default:
      return kind
  }
}

/** Label of a porcelain status letter from `git status`. */
export function gitCodeLabel(code: string): string {
  switch (code) {
    case 'M':
      return 'Modifié'
    case 'A':
      return 'Ajouté'
    case 'D':
      return 'Supprimé'
    case 'R':
      return 'Renommé'
    case 'C':
      return 'Copié'
    case 'T':
      return 'Type modifié'
    case 'U':
      return 'Conflit'
    case '?':
      return 'Non suivi'
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

export function plural(count: number, one: string, many: string): string {
  return `${count} ${count === 1 ? one : many}`
}
