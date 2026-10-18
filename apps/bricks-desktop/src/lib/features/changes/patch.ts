// Presentation of a unified diff produced by Rust. Nothing is computed here
// beyond splitting the text into lines and numbering them from the hunk
// headers: the patch itself always comes from `git diff` or the journal.
//
// Change proposals in a message are not Git diffs: they may come without
// hunk headers, with several files in one text, or with a `--- `/`+++ ` pair
// per file. `parsePatch(text, { loose: true })` and `patchFiles` read them;
// the default reading (the Git views) is unchanged.

export type PatchLineKind = 'add' | 'del' | 'context' | 'hunk' | 'meta'

export interface PatchLine {
  kind: PatchLineKind
  /** Line text as git printed it, including the leading `+`, `-` or space. */
  text: string
  oldNo: number | null
  newNo: number | null
}

export interface ParsePatchOptions {
  /**
   * Proposal reading. A `+` or `-` line outside any hunk counts as an added or
   * removed line when the text has no `@@` at all, and each `--- `/`+++ ` pair
   * outside a hunk starts a new file: its counts are its own.
   */
  loose?: boolean
}

const HUNK_HEADER = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/
/** Same header with the line counts of each side (1 when omitted). */
const HUNK_COUNTS = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/

export function parsePatch(patch: string, options: ParsePatchOptions = {}): PatchLine[] {
  return options.loose ? parseLoose(patch) : parseStrict(patch)
}

function splitLines(patch: string): string[] {
  const lines = patch.split('\n')
  if (lines.length > 0 && lines[lines.length - 1] === '') lines.pop()
  return lines
}

function parseStrict(patch: string): PatchLine[] {
  const lines = splitLines(patch)

  const out: PatchLine[] = []
  let inHunk = false
  let oldNo = 0
  let newNo = 0

  for (const text of lines) {
    if (text.startsWith('diff --git ')) {
      inHunk = false
      out.push({ kind: 'meta', text, oldNo: null, newNo: null })
      continue
    }
    const header = HUNK_HEADER.exec(text)
    if (header) {
      inHunk = true
      oldNo = Number(header[1])
      newNo = Number(header[2])
      out.push({ kind: 'hunk', text, oldNo: null, newNo: null })
      continue
    }
    if (!inHunk) {
      out.push({ kind: 'meta', text, oldNo: null, newNo: null })
      continue
    }
    const sign = text.charAt(0)
    if (sign === '+') {
      out.push({ kind: 'add', text, oldNo: null, newNo })
      newNo++
    } else if (sign === '-') {
      out.push({ kind: 'del', text, oldNo, newNo: null })
      oldNo++
    } else if (sign === ' ' || text === '') {
      out.push({ kind: 'context', text, oldNo, newNo })
      oldNo++
      newNo++
    } else {
      // e.g. `\ No newline at end of file`
      out.push({ kind: 'meta', text, oldNo: null, newNo: null })
    }
  }
  return out
}

function meta(text: string): PatchLine {
  return { kind: 'meta', text, oldNo: null, newNo: null }
}

/**
 * Proposal reading. Inside a hunk, the header's line counts say where it ends,
 * so a removed line that starts with `-- ` is not taken for a file header.
 */
function parseLoose(patch: string): PatchLine[] {
  const lines = splitLines(patch)
  const hasHunks = lines.some((text) => HUNK_HEADER.test(text))

  const out: PatchLine[] = []
  let oldNo = 0
  let newNo = 0
  let oldLeft = 0
  let newLeft = 0

  for (let i = 0; i < lines.length; i++) {
    const text = lines[i]
    // A `diff --git` line is never a hunk line: it ends whatever hunk was open.
    if (text.startsWith('diff --git ')) {
      oldLeft = 0
      newLeft = 0
      out.push(meta(text))
      continue
    }
    const inHunk = oldLeft > 0 || newLeft > 0
    if (!inHunk && text.startsWith('--- ') && (lines[i + 1] ?? '').startsWith('+++ ')) {
      out.push(meta(text), meta(lines[i + 1]))
      i++
      continue
    }
    const header = HUNK_COUNTS.exec(text)
    if (header) {
      oldNo = Number(header[1])
      newNo = Number(header[3])
      oldLeft = header[2] === undefined ? 1 : Number(header[2])
      newLeft = header[4] === undefined ? 1 : Number(header[4])
      out.push({ kind: 'hunk', text, oldNo: null, newNo: null })
      continue
    }
    if (inHunk) {
      const sign = text.charAt(0)
      if (sign === '+') {
        out.push({ kind: 'add', text, oldNo: null, newNo })
        newNo++
        newLeft = Math.max(0, newLeft - 1)
      } else if (sign === '-') {
        out.push({ kind: 'del', text, oldNo, newNo: null })
        oldNo++
        oldLeft = Math.max(0, oldLeft - 1)
      } else if (sign === ' ' || text === '') {
        out.push({ kind: 'context', text, oldNo, newNo })
        oldNo++
        newNo++
        oldLeft = Math.max(0, oldLeft - 1)
        newLeft = Math.max(0, newLeft - 1)
      } else {
        out.push(meta(text))
      }
      continue
    }
    // Outside a hunk: a bare `+`/`-` line is a change only when the text has no hunks.
    if (!hasHunks && text.startsWith('+') && !text.startsWith('+++ ')) {
      out.push({ kind: 'add', text, oldNo: null, newNo: null })
    } else if (!hasHunks && text.startsWith('-') && !text.startsWith('--- ')) {
      out.push({ kind: 'del', text, oldNo: null, newNo: null })
    } else {
      out.push(meta(text))
    }
  }
  return out
}

/** Added and removed line counts of a patch, from its rendered lines. */
export function countPatch(patch: string): { added: number; removed: number } {
  let added = 0
  let removed = 0
  for (const line of parsePatch(patch)) {
    if (line.kind === 'add') added++
    else if (line.kind === 'del') removed++
  }
  return { added, removed }
}

// ─── Files of a proposal ─────────────────────────────────────────────────────

export interface PatchFile {
  /** Path relative to the project, `/` separated; null when the text names none. */
  path: string | null
  /** The text of this file's section, ending with a newline. */
  patch: string
  additions: number
  deletions: number
  binary: boolean
}

interface Section {
  lines: string[]
  git: boolean
  paired: boolean
}

/**
 * The text split by file: a `diff --git` line starts a file; without it, each
 * `--- `/`+++ ` pair after the first one of a section starts a file.
 */
function splitSections(lines: string[]): Section[] {
  const sections: Section[] = []
  let current: Section | undefined
  // Lines still expected in the hunk being read (from its header counts): inside a
  // hunk, `--- x` and `+++ y` are removed and added lines, not a file header.
  let oldLeft = 0
  let newLeft = 0
  for (let i = 0; i < lines.length; i++) {
    const text = lines[i]
    if (text.startsWith('diff --git ')) {
      current = { lines: [text], git: true, paired: false }
      sections.push(current)
      oldLeft = 0
      newLeft = 0
      continue
    }
    const inHunk = oldLeft > 0 || newLeft > 0
    if (!inHunk && text.startsWith('--- ') && (lines[i + 1] ?? '').startsWith('+++ ')) {
      if (!current || (current.paired && !current.git)) {
        current = { lines: [], git: false, paired: false }
        sections.push(current)
      }
      current.paired = true
      current.lines.push(text, lines[i + 1])
      i++
      continue
    }
    if (!current) {
      current = { lines: [], git: false, paired: false }
      sections.push(current)
    }
    const header = HUNK_COUNTS.exec(text)
    if (header) {
      oldLeft = header[2] === undefined ? 1 : Number(header[2])
      newLeft = header[4] === undefined ? 1 : Number(header[4])
    } else if (inHunk) {
      const sign = text.charAt(0)
      if (sign === '+') newLeft = Math.max(0, newLeft - 1)
      else if (sign === '-') oldLeft = Math.max(0, oldLeft - 1)
      else if (sign === ' ' || text === '') {
        oldLeft = Math.max(0, oldLeft - 1)
        newLeft = Math.max(0, newLeft - 1)
      }
    }
    current.lines.push(text)
  }
  return sections.filter((section) => section.lines.some((line) => line.trim() !== ''))
}

const ESCAPES: Record<string, string> = { t: '\t', n: '\n', '"': '"', '\\': '\\' }

/** A path as Git quotes it (`"…"` with C escapes and octal bytes), unquoted. */
function unquotePath(value: string): string {
  if (value.length < 2 || !value.startsWith('"') || !value.endsWith('"')) return value
  const encoder = new TextEncoder()
  const bytes: number[] = []
  for (const match of value.slice(1, -1).matchAll(/\\([0-7]{3}|.)|[^\\]+/gs)) {
    const escape = match[1]
    if (escape === undefined) bytes.push(...encoder.encode(match[0]))
    else if (/^[0-7]{3}$/.test(escape)) bytes.push(Number.parseInt(escape, 8))
    else bytes.push(...encoder.encode(ESCAPES[escape] ?? escape))
  }
  return new TextDecoder().decode(Uint8Array.from(bytes))
}

/** `a/` or `b/` of a Git header name, without it. */
function withoutSide(path: string): string {
  return path.replace(/^[ab]\//, '')
}

/** The name of a `--- ` or `+++ ` header, before the first hunk; null for `/dev/null`. */
function headerPath(lines: string[], prefix: '--- ' | '+++ '): string | null {
  for (const line of lines) {
    if (line.startsWith('@@')) break
    if (!line.startsWith(prefix)) continue
    const name = unquotePath(line.slice(prefix.length).split('\t')[0].trim())
    return name === '/dev/null' || name === '' ? null : withoutSide(name)
  }
  return null
}

/** The `b/` name of a `diff --git a/… b/…` line. */
function gitHeaderPath(lines: string[]): string | null {
  const line = lines.find((text) => text.startsWith('diff --git '))
  if (!line) return null
  const rest = line.slice('diff --git '.length)
  const at = Math.max(rest.lastIndexOf(' "b/'), rest.lastIndexOf(' b/'))
  if (at < 0) return null
  return withoutSide(unquotePath(rest.slice(at + 1)))
}

function pathOf(lines: string[]): string | null {
  return headerPath(lines, '+++ ') ?? headerPath(lines, '--- ') ?? gitHeaderPath(lines)
}

/**
 * The files of a proposal or of a multi-file diff: one entry per section, in
 * order, with its own path and its own added and removed counts.
 */
export function patchFiles(text: string): PatchFile[] {
  return splitSections(splitLines(text)).map((section) => {
    const body = section.lines.join('\n')
    let additions = 0
    let deletions = 0
    for (const line of parseLoose(body)) {
      if (line.kind === 'add') additions++
      else if (line.kind === 'del') deletions++
    }
    return {
      path: pathOf(section.lines),
      patch: `${body}\n`,
      additions,
      deletions,
      binary: section.lines.some(
        (line) => line.startsWith('Binary files ') || line === 'GIT binary patch',
      ),
    }
  })
}
