// Bricks syntax on top of marked: math, footnotes and GitHub alerts (tokenizers),
// file references (post-processing of text tokens), heading slugs and link labels.
// Renderers live in render.ts. Pure apart from the marked objects it hands out.
import { Tokenizer, type Lexer, type Token, type TokenizerExtension, type Tokens } from 'marked'
import { findFileReferences } from './fileref-parse'

/** Any marked token, read loosely: the Bricks fields are not in marked's types. */
export type Tok = Tokens.Generic

// ---------------------------------------------------------------------------
// Labels, slugs, plain text
// ---------------------------------------------------------------------------

/** Same key as marked for link labels (definitions and references). */
export function normalizeLabel(label: string): string {
  return label.trim().replace(/\s+/g, ' ').toLowerCase().toUpperCase().toLowerCase()
}

const BRACKETED = /\[([^\[\]]*)\]/g

/** Normalized labels a block may refer to: an over-approximation, so no real reference is missed. */
export function labelsOf(raw: string): string[] {
  const labels = new Set<string>()
  for (const match of raw.matchAll(BRACKETED)) {
    const label = normalizeLabel(match[1] ?? '')
    if (label !== '') labels.add(label)
  }
  return [...labels].sort()
}

/** Heading slug: lowercase, accents removed, letters and digits kept, each space a dash, punctuation dropped. */
export function slugify(text: string): string {
  const slug = text
    .normalize('NFKD')
    .replace(/\p{M}/gu, '')
    .toLowerCase()
    .replace(/\s/g, '-')
    .replace(/[^\p{L}\p{N}-]/gu, '')
  return slug === '' ? 'section' : slug
}

/** The text of inline tokens, without markup (links keep their text, images their alt text). */
export function plainText(tokens: readonly Token[] | undefined): string {
  let out = ''
  for (const token of tokens ?? []) {
    const node = token as Tok
    if (Array.isArray(node.tokens)) out += plainText(node.tokens)
    else if (node.type === 'br') out += ' '
    else if (typeof node.text === 'string') out += node.text
    else if (node.type === 'footnoteRef') out += node.raw
  }
  return out
}

/** Calls `visit` on every inline and block node, in document order. Link and image children are not visited (rendered from their raw text). */
export function forEachNode(tokens: readonly Token[], visit: (node: Tok) => void): void {
  for (const token of tokens) {
    const node = token as Tok
    visit(node)
    if (node.type === 'link' || node.type === 'image') continue
    if (Array.isArray(node.tokens)) forEachNode(node.tokens, visit)
    if (Array.isArray(node.items)) {
      for (const item of node.items as Tok[]) forEachNode(item.tokens ?? [], visit)
    }
    if (node.type === 'table') {
      for (const cell of (node.header ?? []) as Tok[]) forEachNode(cell.tokens ?? [], visit)
      for (const row of (node.rows ?? []) as Tok[][]) {
        for (const cell of row) forEachNode(cell.tokens ?? [], visit)
      }
    }
  }
}

/** Language of a fence: its first word, lowercased. */
export function fenceLanguage(lang: string | undefined): string {
  return (lang ?? '').trim().split(/\s+/)[0]?.toLowerCase() ?? ''
}

/** True when a fenced code block has its closing fence (a streaming fence is not complete). */
export function fenceClosed(raw: string): boolean {
  const lines = raw.replace(/\n+$/, '').split('\n')
  if (lines.length < 2) return false
  const opener = /^ {0,3}(`{3,}|~{3,})/.exec(lines[0] ?? '')
  const closer = /^ {0,3}(`{3,}|~{3,})[ \t]*$/.exec(lines[lines.length - 1] ?? '')
  if (!opener || !closer) return false
  const open = opener[1] ?? ''
  const close = closer[1] ?? ''
  return close[0] === open[0] && close.length >= open.length
}

/** Mark of the internal labels given to repeated link definitions (never a label of a real definition). */
export const DUPLICATE_MARK = '\u0000'

/**
 * Definition tokenizer override (installed on the marked instance). marked drops a definition
 * whose label is already defined, which loses its source text: the first definition wins, as
 * before, and the repeated one keeps its token under an internal label, so its raw stays whole
 * and the source stays aligned with the tokens. It renders nothing.
 */
export function linkDefinition(this: Tokenizer, src: string): Tokens.Def | undefined {
  const token = Tokenizer.prototype.def.call(this, src)
  if (!token) return undefined
  const links = (this as unknown as { lexer: Lexer }).lexer.tokens.links
  if (links[token.tag] === undefined) return token
  // One key per repeated definition: the count of keys grows at each one.
  return { ...token, tag: `${token.tag}${DUPLICATE_MARK}${Object.keys(links).length}` }
}

// ---------------------------------------------------------------------------
// Math: $…$, $$…$$, \(…\), \[…\]
// ---------------------------------------------------------------------------

/** Index of the first unescaped `closer` in [from, limit), or -1. A backslash escapes the next character. */
function findCloser(src: string, from: number, closer: string, limit = src.length): number {
  for (let i = from; i < limit; i++) {
    if (src.startsWith(closer, i)) return i
    if (src[i] === '\\') i += 1
  }
  return -1
}

function mathToken(type: 'inlineMath' | 'blockMath', raw: string, tex: string, display: boolean, open: string, close: string): Tok {
  return { type, raw, text: tex, display, open, close }
}

/**
 * Inline math. `$x$` follows the pandoc rule: the opening dollar is not followed by a space,
 * the closing one is not preceded by a space nor followed by a digit (so `$5 et $10` is text).
 * `$$…$$` and `\[…\]` are display math inside a paragraph; `\(…\)` is inline. `\$` is a dollar.
 * An expression that is not closed stays text.
 */
export const inlineMathExtension: TokenizerExtension = {
  name: 'inlineMath',
  level: 'inline',
  start(src: string) {
    const found = /\$|\\[(\[]/.exec(src)
    return found ? found.index : undefined
  },
  tokenizer(src: string) {
    if (src.startsWith('$$')) {
      const end = findCloser(src, 2, '$$')
      if (end < 0 || src.slice(2, end).trim() === '') return undefined
      return mathToken('inlineMath', src.slice(0, end + 2), src.slice(2, end), true, '$$', '$$')
    }
    if (src.startsWith('\\(') || src.startsWith('\\[')) {
      const display = src[1] === '['
      const close = display ? '\\]' : '\\)'
      const end = findCloser(src, 2, close)
      if (end < 0 || src.slice(2, end).trim() === '') return undefined
      return mathToken('inlineMath', src.slice(0, end + close.length), src.slice(2, end), display, src.slice(0, 2), close)
    }
    if (src[0] === '$') {
      const next = src[1]
      if (next === undefined || /\s/.test(next)) return undefined
      for (let i = 2; i < src.length; i++) {
        if (src[i] === '\\') {
          i += 1
          continue
        }
        if (src[i] === '$' && !/\s/.test(src[i - 1] ?? '') && !/[0-9]/.test(src[i + 1] ?? '')) {
          return mathToken('inlineMath', src.slice(0, i + 1), src.slice(1, i), false, '$', '$')
        }
      }
    }
    return undefined
  },
}

/** Block math: `$$` or `\[` opening a block (up to three spaces of indent), closed on a later line, without a blank line inside. */
export const blockMathExtension: TokenizerExtension = {
  name: 'blockMath',
  level: 'block',
  tokenizer(src: string) {
    const opening = /^ {0,3}(\$\$|\\\[)/.exec(src)
    if (!opening) return undefined
    const open = opening[1] ?? ''
    const close = open === '$$' ? '$$' : '\\]'
    const bodyStart = opening[0].length
    const blank = /\n[ \t]*\n/.exec(src.slice(bodyStart))
    const limit = blank ? bodyStart + blank.index : src.length
    const end = findCloser(src, bodyStart, close, limit)
    if (end < 0) return undefined
    const rest = /^[ \t]*(?:\n|$)/.exec(src.slice(end + close.length))
    if (!rest) return undefined
    const tex = src.slice(bodyStart, end)
    if (tex.trim() === '') return undefined
    const raw = src.slice(0, end + close.length + rest[0].length)
    return mathToken('blockMath', raw, tex.trim(), true, open, close)
  },
}

// ---------------------------------------------------------------------------
// Footnotes: [^id] and the definition [^id]: text (continuation lines indented)
// ---------------------------------------------------------------------------

const FOOTNOTE_DEF = /^\[\^([^\]\s]+)\]:[ \t]*/
const INDENTED_LINE = /^(?: {2,}|\t)\S/

export const footnoteRefExtension: TokenizerExtension = {
  name: 'footnoteRef',
  level: 'inline',
  start(src: string) {
    const index = src.indexOf('[^')
    return index < 0 ? undefined : index
  },
  tokenizer(src: string) {
    const match = /^\[\^([^\]\s]+)\]/.exec(src)
    if (!match) return undefined
    return { type: 'footnoteRef', raw: match[0], id: match[1] } as Tok
  },
}

export const footnoteDefExtension: TokenizerExtension = {
  name: 'footnoteDef',
  level: 'block',
  tokenizer(src: string) {
    const head = FOOTNOTE_DEF.exec(src)
    if (!head) return undefined
    const firstEnd = src.indexOf('\n')
    const lineEnd = firstEnd < 0 ? src.length : firstEnd
    const lines = [src.slice(head[0].length, lineEnd)]
    let consumed = firstEnd < 0 ? src.length : firstEnd + 1
    let pos = consumed
    let blanks: string[] = []
    while (pos < src.length) {
      const newline = src.indexOf('\n', pos)
      const end = newline < 0 ? src.length : newline
      const line = src.slice(pos, end)
      const next = newline < 0 ? src.length : newline + 1
      if (/^[ \t]*$/.test(line)) {
        blanks.push('')
        pos = next
        continue
      }
      if (!INDENTED_LINE.test(line)) break
      lines.push(...blanks, line.replace(/^(?: {2,}|\t)/, ''))
      blanks = []
      consumed = next
      pos = next
    }
    return {
      type: 'footnoteDef',
      raw: src.slice(0, consumed),
      id: head[1],
      text: lines.join('\n').trim(),
    } as Tok
  },
}

// ---------------------------------------------------------------------------
// GitHub alerts: a blockquote whose first line is [!NOTE], [!TIP], [!IMPORTANT], [!WARNING] or [!CAUTION]
// ---------------------------------------------------------------------------

const ALERT_OPENER = /^ {0,3}>[ \t]?\[!(NOTE|TIP|IMPORTANT|WARNING|CAUTION)\][ \t]*(?:\n|$)/i
const ALERT_MARKER = /^\[!(NOTE|TIP|IMPORTANT|WARNING|CAUTION)\][ \t]*(?:\n|$)/i

/**
 * Blockquote tokenizer override (installed on the marked instance): a blockquote that opens
 * with an alert marker becomes an `alert` token with its `variant` (lowercased); the marker
 * line is removed and the rest is lexed as blocks. Anything else is left to marked (false).
 */
export function alertBlockquote(this: Tokenizer, src: string): Tokens.Blockquote | false | undefined {
  if (!ALERT_OPENER.test(src)) return false
  const quote = Tokenizer.prototype.blockquote.call(this, src)
  if (!quote) return undefined
  const marker = ALERT_MARKER.exec(quote.text)
  if (!marker) return quote
  const lexer = (this as unknown as { lexer: Lexer }).lexer
  const rest = quote.text.slice(marker[0].length)
  const top = lexer.state.top
  lexer.state.top = true
  const tokens = lexer.blockTokens(rest, [])
  lexer.state.top = top
  // An alert is a token of its own: marked renders it through the `alert` renderer.
  return { type: 'alert', raw: quote.raw, text: rest, variant: (marker[1] ?? '').toLowerCase(), tokens } as unknown as Tokens.Blockquote
}

// ---------------------------------------------------------------------------
// File references: split text tokens (never link or code content)
// ---------------------------------------------------------------------------

/** Replaces the bare file references of every text token by `fileRef` tokens. Returns the new list; containers are updated in place. */
export function refineTokens(tokens: Token[]): Token[] {
  const out: Token[] = []
  for (const token of tokens) {
    const node = token as Tok
    if (node.type === 'text' && !Array.isArray(node.tokens)) {
      out.push(...splitText(node))
      continue
    }
    if (node.type !== 'link' && node.type !== 'image') {
      if (Array.isArray(node.tokens)) node.tokens = refineTokens(node.tokens)
      if (Array.isArray(node.items)) {
        for (const item of node.items as Tok[]) {
          if (Array.isArray(item.tokens)) item.tokens = refineTokens(item.tokens)
        }
      }
      if (node.type === 'table') {
        for (const cell of (node.header ?? []) as Tok[]) cell.tokens = refineTokens(cell.tokens ?? [])
        for (const row of (node.rows ?? []) as Tok[][]) {
          for (const cell of row) cell.tokens = refineTokens(cell.tokens ?? [])
        }
      }
    }
    out.push(token)
  }
  return out
}

function splitText(node: Tok): Token[] {
  const text = typeof node.text === 'string' ? node.text : ''
  const found = findFileReferences(text)
  if (found.length === 0) return [node as Token]
  const pieces: Token[] = []
  let last = 0
  for (const match of found) {
    if (match.start > last) {
      const plain = text.slice(last, match.start)
      pieces.push({ type: 'text', raw: plain, text: plain } as Tok as Token)
    }
    const label = text.slice(match.start, match.end)
    pieces.push({ type: 'fileRef', raw: label, text: label, ref: match.ref } as Tok as Token)
    last = match.end
  }
  if (last < text.length) {
    const plain = text.slice(last)
    pieces.push({ type: 'text', raw: plain, text: plain } as Tok as Token)
  }
  return pieces
}
