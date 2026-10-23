// File, folder and language icons: one synchronous, local resolution for
// every surface. The rules and SVGs are the app's own copy of the Logoscrap
// catalog (`public/file-icons`, imported by `scripts/import_file_icons.py`);
// nothing is fetched, and a URL only ever names one of those files.

import rules from './rules.json' with { type: 'json' }

export type IconKind = 'file' | 'folder' | 'root'

export interface IconQuery {
  /** A path or a bare name (Windows or Unix separators). */
  path?: string | null
  kind?: IconKind
  /** Folders: open or closed. */
  open?: boolean
  /** A code fence label, used only when no path is known. */
  language?: string | null
}

const icons: Record<string, string> = rules.icons
const exact: Record<string, string> = rules.exact
const extensions: Record<string, string> = rules.extensions
const folders: Record<string, string> = rules.folders
const aliases: Record<string, string> = rules.aliases

/** A rule of `map` for `key`, never an inherited property (`constructor`…). */
function own(map: Record<string, string>, key: string): string | undefined {
  return Object.hasOwn(map, key) ? map[key] : undefined
}

/** The last segment of a path, whatever its separators. */
export function baseName(path: string): string {
  const parts = path.replace(/\\/g, '/').replace(/\/+$/, '').split('/')
  return parts[parts.length - 1] ?? ''
}

/** A file: exact name, then the longest dotted suffix, then generic. */
export function fileIconId(path: string): string {
  const lower = baseName(path).toLowerCase()
  const named = own(exact, lower)
  if (named) return named
  const parts = lower.split('.')
  for (let i = 1; i < parts.length; i++) {
    const suffix = parts.slice(i).join('.').replace(/^\.+/, '')
    const id = own(extensions, suffix)
    if (id) return id
  }
  return rules.generic
}

/** A folder: its name's icon, else open/closed (the root has its own). */
export function folderIconId(path: string, open = false, root = false): string {
  if (root) return rules.folder.root
  const named = own(folders, baseName(path).toLowerCase())
  if (named) return named
  return open ? rules.folder.open : rules.folder.closed
}

/** A code fence label (already normalised: lowercase first word): an
 *  alias, else a label that is itself an extension; `null` when unknown. */
export function languageIconId(label: string | null | undefined): string | null {
  if (!label) return null
  const target = own(aliases, label)
  if (target) return fileIconId(target)
  return own(extensions, label) ?? null
}

/** The icon of a query; a known path wins over a language. */
export function iconId(q: IconQuery): string {
  const kind = q.kind ?? 'file'
  if (kind !== 'file') return folderIconId(q.path ?? '', q.open, kind === 'root')
  if (q.path) return fileIconId(q.path)
  return languageIconId(q.language) ?? rules.generic
}

export const GENERIC_ICON_ID: string = rules.generic

export function isIconId(id: string): boolean {
  return Object.hasOwn(icons, id)
}

/** The URL of a catalog icon (the generic one for an unknown id). */
export function iconUrl(id: string): string {
  const file = own(icons, id) ?? icons[rules.generic]
  return `${import.meta.env?.BASE_URL ?? '/'}file-icons/${file}`
}

/** The same image as `FileIcon`, for HTML built outside a component (the
 *  code block headers). Only catalog ids produce a source. */
export function iconImgHtml(id: string): string {
  return `<img class="file-icon" src="${iconUrl(id)}" alt="" aria-hidden="true" width="16" height="16" draggable="false" decoding="async">`
}
