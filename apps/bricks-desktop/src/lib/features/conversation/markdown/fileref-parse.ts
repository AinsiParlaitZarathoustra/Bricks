// File references of assistant messages: a Markdown link whose target is a project
// file, or a bare path in text. Pure (no DOM, no IPC): unit-tested in
// tests/unit/markdown-extensions.test.ts. Paths are relative to the conversation's project.
import type { FileReference } from './types'

/** A URI scheme (`https:`, `mailto:`, `file:`…) or a Windows drive. */
const SCHEME = /^[a-z][a-z0-9+.-]*:/i
/** Characters a project path never contains: controls, backslash, wildcards, quotes, brackets, pipes. */
const FORBIDDEN = /[\u0000-\u001f\u007f\\?*<>|"]/
/** A file extension on the last segment: a name character, a dot, then a letter-led extension. */
const EXTENSION = /[^.]\.[A-Za-z][A-Za-z0-9]{0,9}$/
/** A trailing `:line` or `:line-endLine` of a text reference or of an href path. */
const LINE_SUFFIX = /:(\d+)(?:-(\d+))?$/
/** A fragment `#L3`, `#L3-L9` (columns are accepted and ignored). */
const HREF_LINES = /^#L(\d+)(?:C\d+)?(?:-L?(\d+)(?:C\d+)?)?$/
/** A path segment of a bare reference: letters, digits and a few symbols, dots inside. */
const SEGMENT = String.raw`[\p{L}\p{N}_~+@-]+(?:\.[\p{L}\p{N}_~+@-]+)*`
/** Optional `./`, directories (possibly dot-led), then the file name (possibly dot-led). */
const PATH = String.raw`(?:\.\/)?(?:\.?${SEGMENT}\/)*\.?${SEGMENT}`
/** Candidate bare references in text: not glued to a word, a URL, an address or a path. */
const TEXT_CANDIDATE = new RegExp(
  String.raw`(?<![\p{L}\p{N}_/\\:.@~+-])${PATH}(?::\d+(?:-\d+)?)?(?![\p{L}\p{N}_/-]|\.[\p{L}\p{N}]|:\d)`,
  'gu',
)
/** A first segment that is a host name (`example.com/…`, `www.…`): not a project directory. */
const HOST_FIRST_SEGMENT = /^(?:www\.|[\p{L}\p{N}-]+\.[A-Za-z]{2,6}$)/iu
/** A bare name that reads as a web host (`example.com`, `www.…`), not as a file. */
const HOST_NAME = /^www\.|\.(?:com|org|net|io|dev|app|fr|de|uk|edu|gov|info|co|eu|ai|us|me)$/i

type Lines = { line: number; endLine?: number }

export interface FileReferenceMatch {
  start: number
  end: number
  ref: FileReference
}

/** `decodeURIComponent`, or null for a malformed escape. */
export function safeDecode(text: string): string | null {
  try {
    return decodeURIComponent(text)
  } catch {
    return null
  }
}

/**
 * Parses a file reference. `from: 'href'` reads a Markdown link target (`src/a.ts#L3-L9`,
 * `src/a%20b.ts`, `src/a.ts:42`); `from: 'text'` reads a bare path (`src/a.ts:42-50`).
 * Returns null when the input is not a project file reference: a scheme, an absolute or
 * fragment target, a `..` segment, no extension, or neither a directory nor a line.
 */
export function parseFileReference(input: string, from: 'href' | 'text' = 'text'): FileReference | null {
  return from === 'href' ? parseHref(input) : parseText(input)
}

/** Every bare file reference of a text, in order, never overlapping. */
export function findFileReferences(text: string): FileReferenceMatch[] {
  const found: FileReferenceMatch[] = []
  for (const match of text.matchAll(TEXT_CANDIDATE)) {
    const start = match.index ?? 0
    const ref = parseFileReference(match[0], 'text')
    if (ref) found.push({ start, end: start + match[0].length, ref })
  }
  return found
}

function parseHref(href: string): FileReference | null {
  if (SCHEME.test(href) || href.startsWith('/') || href.startsWith('#')) return null
  let base = href
  let lines: Lines | undefined
  const hash = href.indexOf('#')
  if (hash >= 0) {
    const fragment = HREF_LINES.exec(href.slice(hash))
    if (!fragment) return null
    base = href.slice(0, hash)
    lines = lineRange(fragment[1], fragment[2])
    if (!lines) return null
  }
  const decoded = safeDecode(base)
  if (decoded === null) return null
  let path = decoded
  if (lines === undefined) {
    const suffix = LINE_SUFFIX.exec(path)
    if (suffix) {
      lines = lineRange(suffix[1], suffix[2])
      if (!lines) return null
      path = path.slice(0, suffix.index)
    }
  }
  return finish(path, lines, 'href')
}

function parseText(text: string): FileReference | null {
  let path = text
  let lines: Lines | undefined
  const suffix = LINE_SUFFIX.exec(text)
  if (suffix) {
    lines = lineRange(suffix[1], suffix[2])
    if (!lines) return null
    path = text.slice(0, suffix.index)
  }
  return finish(path, lines, 'text')
}

function lineRange(first: string, last: string | undefined): Lines | undefined {
  const line = Number(first)
  if (!Number.isSafeInteger(line) || line < 1) return undefined
  if (last === undefined) return { line }
  const endLine = Number(last)
  if (!Number.isSafeInteger(endLine) || endLine < line) return undefined
  return { line, endLine }
}

function finish(rawPath: string, lines: Lines | undefined, from: 'href' | 'text'): FileReference | null {
  const path = projectPath(rawPath)
  if (path === null) return null
  // A bare name needs a line, except a link target that is not a host name: `[notes](NOTES.md)`.
  if (lines === undefined && !path.includes('/') && (from === 'text' || HOST_NAME.test(path))) return null
  return lines === undefined
    ? { path }
    : lines.endLine === undefined
      ? { path, line: lines.line }
      : { path, line: lines.line, endLine: lines.endLine }
}

function projectPath(raw: string): string | null {
  const path = raw.startsWith('./') ? raw.slice(2) : raw
  // A home path (`~/…`) or a drive (`C:…`) is outside the project: never a reference.
  if (path === '' || path.startsWith('~') || /^[A-Za-z]:/.test(path) || FORBIDDEN.test(path)) return null
  const segments = path.split('/')
  if (segments.some((segment) => segment === '' || segment === '.' || segment === '..')) return null
  if (!EXTENSION.test(segments[segments.length - 1] ?? '')) return null
  if (segments.length > 1 && HOST_FIRST_SEGMENT.test(segments[0] ?? '')) return null
  return path
}
