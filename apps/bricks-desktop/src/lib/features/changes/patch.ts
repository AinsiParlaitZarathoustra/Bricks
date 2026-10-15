// Presentation of a unified diff produced by Rust. Nothing is computed here
// beyond splitting the text into lines and numbering them from the hunk
// headers: the patch itself always comes from `git diff` or the journal.

export type PatchLineKind = 'add' | 'del' | 'context' | 'hunk' | 'meta'

export interface PatchLine {
  kind: PatchLineKind
  /** Line text as git printed it, including the leading `+`, `-` or space. */
  text: string
  oldNo: number | null
  newNo: number | null
}

const HUNK_HEADER = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/

export function parsePatch(patch: string): PatchLine[] {
  const lines = patch.split('\n')
  if (lines.length > 0 && lines[lines.length - 1] === '') lines.pop()

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
