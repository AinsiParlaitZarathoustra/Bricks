// Links in user messages. The text is plain (no Markdown): http(s) addresses and
// `www.` addresses become link parts, everything else stays text. Code (fenced or
// inline) is never linked. The result is rendered with Svelte markup (text nodes
// and anchors), never as HTML. Pure: unit-tested (tests/unit/links-message.test.ts).

/** Only these schemes are rendered as links and may be opened. */
export function isOpenableUrl(href: string): boolean {
  try {
    const { protocol } = new URL(href)
    return protocol === 'https:' || protocol === 'http:' || protocol === 'mailto:'
  } catch {
    return false
  }
}

export type MessagePart =
  | { kind: 'text'; text: string }
  | { kind: 'link'; text: string; href: string }

/** Closed fenced blocks and inline code spans. */
const CODE_SPAN = /```[\s\S]*?```|`[^`\n]+`/g
/** A web address start (`https://`, `http://` or `www.`) not glued to a word, path or address. */
const ADDRESS = /(?<![\p{L}\p{N}_@./-])(?:https?:\/\/|www\.)[^\s<>"`«»“”]+/giu

/** Characters that end a sentence rather than the address. */
const TRAILING_PUNCTUATION = '.,;:!?\'’'

function countOf(text: string, char: string): number {
  let n = 0
  for (const c of text) if (c === char) n += 1
  return n
}

/**
 * The address without the punctuation that follows it in a sentence: a final
 * closing bracket is kept only when the address opens it.
 */
export function trimAddress(raw: string): string {
  let url = raw
  while (url.length > 0) {
    const last = url[url.length - 1]
    if (TRAILING_PUNCTUATION.includes(last)) {
      url = url.slice(0, -1)
    } else if (last === ')' && countOf(url, ')') > countOf(url, '(')) {
      url = url.slice(0, -1)
    } else if (last === ']' && countOf(url, ']') > countOf(url, '[')) {
      url = url.slice(0, -1)
    } else {
      break
    }
  }
  return url
}

/** The href of a trimmed address, or null when it is not a usable web address. */
export function hrefFor(address: string): string | null {
  const href = /^www\./i.test(address) ? `https://${address}` : address
  if (!/^(?:https?:\/\/|www\.)/i.test(address)) return null
  const rest = href.replace(/^https?:\/\//i, '')
  if (rest === '' || /^[/?#]/.test(rest)) return null
  return isOpenableUrl(href) ? href : null
}

/** Splits a user message into text and link parts (in order, nothing lost). */
export function messageParts(text: string): MessagePart[] {
  const parts: MessagePart[] = []
  let last = 0
  for (const code of text.matchAll(CODE_SPAN)) {
    const start = code.index ?? 0
    pushText(parts, linkify(text.slice(last, start)))
    pushText(parts, [{ kind: 'text', text: code[0] }])
    last = start + code[0].length
  }
  pushText(parts, linkify(text.slice(last)))
  return parts
}

/** Link parts of a text without code spans. */
function linkify(chunk: string): MessagePart[] {
  const out: MessagePart[] = []
  let last = 0
  for (const match of chunk.matchAll(ADDRESS)) {
    const start = match.index ?? 0
    const url = trimAddress(match[0])
    const href = hrefFor(url)
    if (href === null) continue
    if (start > last) out.push({ kind: 'text', text: chunk.slice(last, start) })
    out.push({ kind: 'link', text: url, href })
    // The punctuation trimmed off the address stays text.
    if (url.length < match[0].length) out.push({ kind: 'text', text: match[0].slice(url.length) })
    last = start + match[0].length
  }
  if (last < chunk.length) out.push({ kind: 'text', text: chunk.slice(last) })
  return out
}

/** Adjacent text parts are merged; empty texts are dropped. */
function pushText(parts: MessagePart[], more: MessagePart[]): void {
  for (const part of more) {
    if (part.kind === 'text' && part.text === '') continue
    const previous = parts[parts.length - 1]
    if (part.kind === 'text' && previous?.kind === 'text') {
      previous.text += part.text
    } else {
      parts.push({ ...part })
    }
  }
}
