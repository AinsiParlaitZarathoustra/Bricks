// `@` mentions in the composer: detection of the token being typed, its removal
// once the entry is attached, and the path and kind of a project entry. Pure
// (no runes, no IPC): unit-tested. Owner: Haiku 3 (Sprint 14.1, @ mentions).

import { isImagePath } from './attachments'

export interface MentionToken {
  /** Index of the `@`. */
  start: number
  /** The caret: the token ends here. */
  end: number
  /** The text typed after the `@`. */
  query: string
}

/**
 * The `@` token the caret is at the end of: an `@` at the start of the text or
 * after a space or line break, followed by a query without spaces. `null` when
 * the caret is not in such a token (an e-mail address, a space typed after it,
 * text still following the caret).
 */
export function mentionAt(text: string, caret: number): MentionToken | null {
  if (caret < 0 || caret > text.length) return null
  const before = text.slice(0, caret)
  const at = before.lastIndexOf('@')
  if (at < 0) return null
  const query = before.slice(at + 1)
  if (/\s/.test(query)) return null
  if (at > 0 && !/\s/.test(text.charAt(at - 1))) return null
  const after = text.slice(caret)
  if (after !== '' && !/^\s/.test(after)) return null
  return { start: at, end: caret, query }
}

/** The text without the token, and the caret where it was. One space that separated the token from the text before it goes too. */
export function removeMention(text: string, token: MentionToken): { text: string; caret: number } {
  const before = text.slice(0, token.start)
  let after = text.slice(token.end)
  if (after.startsWith(' ') && (before === '' || /\s$/.test(before))) after = after.slice(1)
  return { text: before + after, caret: before.length }
}

/** Absolute path of a project entry: the project root, then the entry's `/`-separated relative path. */
export function projectEntryPath(root: string, relative: string): string {
  const windows = root.includes('\\') && !root.includes('/')
  const sep = windows ? '\\' : '/'
  const base = root.replace(/[\\/]+$/, '')
  const tail = relative.replace(/^\/+/, '')
  return `${base}${sep}${windows ? tail.replace(/\//g, '\\') : tail}`
}

/** Attachment kind of a path that exists: a folder, an image (by extension), or a file. */
export function mentionKind(path: string, found: 'file' | 'folder'): 'file' | 'folder' | 'image' {
  if (found === 'folder') return 'folder'
  return isImagePath(path) ? 'image' : 'file'
}
