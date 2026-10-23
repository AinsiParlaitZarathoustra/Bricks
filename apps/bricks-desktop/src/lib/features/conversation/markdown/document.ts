// A message as blocks, updated while it streams. The source is kept whole. Top-level tokens
// before the penultimate non-space token are frozen (final), so an update only lexes the
// tail and re-renders what a dependency changed. The dependencies of a block are its link
// labels (definitions), its heading ids (earlier headings), its footnote numbers and the math
// renderer/macro state in source order. `final` re-lexes everything and reuses every unchanged block, so it equals
// `renderReference` of the same source.
import type { Token } from 'marked'
import { DUPLICATE_MARK, fenceClosed, fenceLanguage, forEachNode, labelsOf, plainText, slugify, type Tok } from './extensions'
import {
  codeShell,
  fenceIconId,
  withCodeIcons,
  escapeHtml,
  finishHtml,
  footnotesHtml,
  highlightCount,
  highlightedBody,
  knownLanguage,
  lexBlocks,
  marked,
  withScope,
  type LinkMap,
  type MathSettings,
} from './render'
import type { MarkdownDocumentOptions, MarkdownHeading, MathRenderer, RenderStats, RenderedBlock } from './types'

/** Output signature of a block rendered progressively: never equal to a dependency signature. */
const OPEN_SIG = 'open'
/**
 * An open code block (streaming) is highlighted in full once its unhighlighted tail passes
 * these bounds; in between, the tail is appended escaped.
 */
const PROGRESSIVE_CHARS = 1500
const PROGRESSIVE_LINES = 40
/** Budget of the render cache, in approximate bytes (two per UTF-16 unit of raw and html). */
const CACHE_BYTES = 8 * 1024 * 1024

/** One rendered block of the cache: the raw and dependency signature it was rendered for. */
interface CachedBlock {
  raw: string
  sig: string
  html: string
  bytes: number
  macros: MacroState | null
}

/** A macro mutation, replayed on a cache hit without running KaTeX again. Values are
 * opaque KaTeX objects (with circular source locations), so never JSON-serialize them. */
interface MacroState {
  version: number
  values: Record<string, string>
}

/** A fence still open at the end of the source (streaming): the last block, always re-rendered while it grows. */
function isOpenFence(entry: Entry): boolean {
  return (
    entry.token !== null &&
    (entry.token as Tok).type === 'code' &&
    /^ {0,3}(`{3,}|~{3,})/.test(entry.raw) &&
    !fenceClosed(entry.raw)
  )
}

/** One top-level token (or a text marked dropped, such as a repeated link definition) with its dependencies. */
interface Entry {
  /** `b<start offset>`: stable while the block keeps its place. */
  key: string
  start: number
  raw: string
  /** Null for a text range marked emitted no token for. */
  token: Token | null
  kind: 'none' | 'html' | 'mermaid' | 'diff'
  footnote: { id: string; text: string } | null
  /** Normalized link labels the block refers to, and the definitions they resolved to when lexed. */
  labels: string[]
  labelSig: string
  headingTokens: Tok[]
  headings: MarkdownHeading[]
  idsSig: string
  /** Footnote references in document order; numbers are set by `resolve`. */
  refs: Tok[]
  fnSig: string
  mathCount: number
  complete: boolean
  output: RenderedBlock | null
  outputSig: string
  macros: MacroState | null
}

function emptyStats(): RenderStats {
  return {
    lexedChars: 0,
    renderedBlocks: 0,
    reusedBlocks: 0,
    highlightCalls: 0,
    mathRenders: 0,
    invalidatedBlocks: 0,
    fullLex: false,
    fallback: false,
    ms: 0,
  }
}

function kindOf(node: Tok | null): Entry['kind'] {
  if (node === null || node.type === 'def' || node.type === 'footnoteDef') return 'none'
  if (node.type === 'code') {
    const language = fenceLanguage(node.lang as string | undefined)
    if (language === 'mermaid') return 'mermaid'
    if (language === 'diff' || language === 'patch') return 'diff'
  }
  return 'html'
}

function makeEntry(start: number, raw: string, token: Token | null): Entry {
  const node = token as Tok | null
  const kind = kindOf(node)
  const entry: Entry = {
    key: `b${start}`,
    start,
    raw,
    token,
    kind,
    footnote: node?.type === 'footnoteDef' ? { id: node.id as string, text: node.text as string } : null,
    labels: [],
    labelSig: '',
    headingTokens: [],
    headings: [],
    idsSig: '',
    refs: [],
    fnSig: '',
    mathCount: 0,
    complete: kind === 'mermaid' || kind === 'diff' ? fenceClosed(raw) : false,
    output: null,
    outputSig: '',
    macros: null,
  }
  if (token !== null && kind === 'html') {
    if (node?.type !== 'code' && node?.type !== 'html') entry.labels = labelsOf(raw)
    forEachNode([token], (inner) => {
      if (inner.type === 'heading') entry.headingTokens.push(inner)
      else if (inner.type === 'footnoteRef') entry.refs.push(inner)
      else if (inner.type === 'inlineMath' || inner.type === 'blockMath') entry.mathCount += 1
    })
  }
  return entry
}

/**
 * End of the source text a token came from, starting at `pos`, or -1. marked may drop the
 * blanks at the end of a line and add a final newline to a raw; those are tolerated here.
 */
function matchEnd(src: string, pos: number, raw: string): number {
  let i = pos
  let j = 0
  while (j < raw.length) {
    const char = raw[j]
    if (i < src.length && src[i] === char) {
      i += 1
      j += 1
    } else if (i < src.length && (src[i] === ' ' || src[i] === '\t')) {
      i += 1
    } else if (char === '\n' && i >= src.length) {
      j += 1
    } else {
      return -1
    }
  }
  return i
}

/**
 * Entries of the top-level tokens of `src` (which starts at absolute offset `base`). Their raws
 * are source slices: they concatenate to `src`. Text no token covers (a repeated link
 * definition, which marked drops) becomes a `none` entry; spaces are not entries.
 */
function entriesOf(tokens: Token[], src: string, base: number): Entry[] {
  const entries: Entry[] = []
  let pos = 0
  for (const token of tokens) {
    const raw = (token as Tok).raw as string
    if (raw === '') continue
    let at = pos
    let end = matchEnd(src, pos, raw)
    if (end < 0) {
      at = src.indexOf(raw, pos)
      if (at < 0) throw new Error('markdown: token outside its source')
      end = at + raw.length
    }
    if (at > pos) entries.push(makeEntry(base + pos, src.slice(pos, at), null))
    if (token.type !== 'space') entries.push(makeEntry(base + at, src.slice(at, end), token))
    pos = end
  }
  if (pos < src.length) entries.push(makeEntry(base + pos, src.slice(pos), null))
  return entries
}

function labelSignature(labels: readonly string[], links: LinkMap): string {
  return labels
    .map((label) => {
      const definition = links[label]
      return definition ? `${label}\u0001${definition.href}\u0001${definition.title ?? ''}` : `${label}\u0001`
    })
    .join('\u0002')
}

function linksSignature(links: LinkMap): string {
  return Object.keys(links)
    .sort()
    .map((label) => {
      const definition = links[label]
      return `${label}\u0001${definition?.href ?? ''}\u0001${definition?.title ?? ''}`
    })
    .join('\u0002')
}

/** `base`, or `base-1`, `base-2`… the first not taken, in document order. */
function uniqueId(base: string, taken: Set<string>): string {
  if (!taken.has(base)) return base
  for (let n = 1; ; n += 1) {
    const candidate = `${base}-${n}`
    if (!taken.has(candidate)) return candidate
  }
}

export class MarkdownDocument {
  private readonly prefix: string
  private readonly cacheLimit: number
  private math: MathRenderer | null = null
  private mathVersion = 0
  private readonly macros: Record<string, string> = {}
  private macroVersion = 0
  private macroSerial = 0
  private notesNeedMath = false
  /** The last rendering of each block (by key), least recently used first, within a byte budget. */
  private readonly cache = new Map<string, CachedBlock>()
  private cacheBytes = 0
  /** The highlighted body of the open code block: the text it covers, and its body. */
  private openCode: { key: string; language: string; text: string; body: string } | null = null
  private lastSource: string | null = null
  /** Every entry of the last update, frozen ones first. */
  private entries: Entry[] = []
  private frozen: Entry[] = []
  /** Offset of the first unfrozen token of the last update. */
  private frozenEnd = 0
  /** Definitions of the frozen blocks: the prefix of every tail lex. */
  private frozenLinks: LinkMap = Object.create(null)
  /** Heading ids taken by the frozen blocks. */
  private frozenIds = new Set<string>()
  private linksSig = ''
  private section: { sig: string; output: RenderedBlock } | null = null
  private lastStats: RenderStats = emptyStats()
  private disposed = false

  constructor(options: MarkdownDocumentOptions) {
    // Ids are built from the prefix: only [a-z0-9-] survives, so any message id is usable.
    this.prefix = options.idPrefix.toLowerCase().replace(/[^a-z0-9-]/g, '-') || 'md'
    this.cacheLimit = Math.max(1, options.cacheLimit ?? 400)
  }

  /** Counters of the last `update`. */
  get stats(): RenderStats {
    return { ...this.lastStats }
  }

  /** True when the message has math and no math renderer is installed yet (KaTeX still loading). */
  get needsMath(): boolean {
    return this.math === null && (this.notesNeedMath || this.entries.some((entry) => entry.mathCount > 0))
  }

  /** The headings in document order; ids are the ids of the HTML. */
  get headings(): MarkdownHeading[] {
    return this.entries.flatMap((entry) => entry.headings.map((heading) => ({ ...heading })))
  }

  /** Installs (or removes, with null) the math renderer. Blocks with math render again. */
  setMathRenderer(renderer: MathRenderer | null): void {
    if (renderer === this.math) return
    this.math = renderer
    this.mathVersion += 1
  }

  /** Releases the cache and the blocks. Later updates return nothing. */
  dispose(): void {
    this.disposed = true
    this.cache.clear()
    this.cacheBytes = 0
    this.openCode = null
    this.entries = []
    this.frozen = []
    this.lastSource = null
    this.clearMacros()
    this.notesNeedMath = false
  }

  /**
   * The blocks of `source`, the whole message so far. `final` re-lexes it entirely (the end of
   * a stream, or a replaced message): blocks that did not change keep their objects.
   */
  update(source: string, final: boolean): RenderedBlock[] {
    if (this.disposed) return []
    const started = performance.now()
    const text = String(source ?? '').replace(/\r\n?/g, '\n')
    const incremental = !final && this.lastSource !== null && text.startsWith(this.lastSource)
    const stats = emptyStats()
    stats.fullLex = !incremental
    const previous = this.entries
    const highlightsBefore = highlightCount()
    const math: MathSettings = { renderer: this.math, macros: this.macros, renders: { count: 0 } }
    // Never throws: an inconsistency falls back to a full lex, and a failing full lex to the
    // source as one escaped paragraph, so no text of the message is lost.
    let blocks: RenderedBlock[] | null = null
    const live = !final
    if (incremental) blocks = this.attempt(text, true, previous, math, stats, live)
    if (blocks === null) {
      stats.fullLex = true
      blocks = this.attempt(text, false, previous, math, stats, live)
    }
    if (blocks === null) {
      stats.fallback = true
      blocks = this.forget(text)
    }
    stats.mathRenders = math.renders.count
    stats.highlightCalls = highlightCount() - highlightsBefore
    stats.ms = performance.now() - started
    this.lastStats = stats
    return blocks
  }

  private attempt(
    text: string,
    incremental: boolean,
    previous: Entry[],
    math: MathSettings,
    stats: RenderStats,
    live: boolean,
  ): RenderedBlock[] | null {
    try {
      return this.resolve(text, incremental, previous, math, stats, live)
    } catch {
      return null
    }
  }

  /** The state of a message that could not be rendered: the next update lexes from scratch. */
  private forget(text: string): RenderedBlock[] {
    this.reset()
    this.entries = []
    this.lastSource = null
    this.linksSig = ''
    this.section = null
    this.openCode = null
    const html = `<p>${escapeHtml(text).replace(/\n/g, '<br>')}</p>`
    return [{ key: 'source', kind: 'html', html }]
  }

  /** `live` is false for the final update: every block is then highlighted and rendered in full. */
  private resolve(
    text: string,
    incremental: boolean,
    previous: Entry[],
    math: MathSettings,
    stats: RenderStats,
    live: boolean,
  ): RenderedBlock[] {
    if (!incremental) this.reset()
    // Every pass starts at the beginning of this source, including source replacements.
    // Cached macro mutations restore the state at their position, never from a later block.
    this.clearMacros()

    // 1. Lex the tail with the definitions of the frozen blocks as prefix.
    const tailSrc = text.slice(this.frozenEnd)
    const lexed = lexBlocks(tailSrc, this.frozenLinks)
    stats.lexedChars += tailSrc.length
    const links = lexed.links
    const linksSig = linksSignature(links)
    const tail = entriesOf(lexed.tokens, tailSrc, this.frozenEnd)

    // 2. A definition added or changed: re-lex the frozen blocks whose labels it touches.
    if (linksSig !== this.linksSig) {
      let headingsChanged = false
      for (const entry of this.frozen) {
        if (entry.labels.length === 0 || labelSignature(entry.labels, links) === entry.labelSig) continue
        headingsChanged = this.relex(entry, links, stats) || headingsChanged
      }
      if (headingsChanged) {
        this.frozenIds = new Set<string>()
        for (const entry of this.frozen) this.assignIds(entry, this.frozenIds)
      }
    }

    // 3. Heading ids of the tail, after the frozen ones. The registry at the freeze point is kept.
    const freezeCount = Math.max(0, tail.length - 2)
    const taken = new Set(this.frozenIds)
    let frozenAfter: Set<string> | null = null
    for (let index = 0; index < tail.length; index += 1) {
      const entry = tail[index]
      if (entry === undefined) continue
      if (index === freezeCount) frozenAfter = new Set(taken)
      entry.labelSig = labelSignature(entry.labels, links)
      this.assignIds(entry, taken)
    }

    // 4. Freeze the tokens before the penultimate non-space one: they are final.
    const moved = tail.splice(0, freezeCount)
    for (const entry of moved) {
      this.frozen.push(entry)
      const node = entry.token as Tok | null
      if (node?.type === 'def' && !(node.tag as string).includes(DUPLICATE_MARK) && this.frozenLinks[node.tag as string] === undefined) {
        this.frozenLinks[node.tag as string] = { href: node.href as string, title: node.title as string | null }
      }
    }
    if (moved.length > 0) this.frozenEnd = tail[0]?.start ?? this.frozenEnd
    this.frozenIds = frozenAfter ?? new Set(taken)

    // 5. Footnote numbers: first reference order over the message, defined notes only.
    const all = [...this.frozen, ...tail]
    const definitions = new Map<string, string>()
    for (const entry of all) {
      if (entry.footnote && !definitions.has(entry.footnote.id)) definitions.set(entry.footnote.id, entry.footnote.text)
    }
    const numbers = new Map<string, number>()
    const counts = new Map<string, number>()
    for (const entry of all) {
      for (const ref of entry.refs) {
        const id = ref.id as string
        if (!definitions.has(id)) {
          ref.number = undefined
          ref.occurrence = undefined
          continue
        }
        let number = numbers.get(id)
        if (number === undefined) {
          number = numbers.size + 1
          numbers.set(id, number)
        }
        const count = (counts.get(id) ?? 0) + 1
        counts.set(id, count)
        ref.number = number
        ref.occurrence = count
      }
      entry.fnSig = entry.refs.map((ref) => (typeof ref.number === 'number' ? `${ref.number}.${ref.occurrence}` : '-')).join(',')
    }

    // 6. Blocks: rendered only when their signature changed.
    const previousByKey = new Map<string, Entry>()
    for (const entry of previous) previousByKey.set(entry.key, entry)
    for (const entry of all) {
      if (entry.kind === 'none') {
        entry.output = null
      } else if (entry.kind === 'html') {
        this.renderEntry(entry, previousByKey, math, stats, live)
      } else {
        this.renderSource(entry, previousByKey)
      }
    }

    // 7. Footnote section, the last block.
    const notes = [...numbers.keys()].map((id) => ({ id, text: definitions.get(id) ?? '', count: counts.get(id) ?? 0 }))
    this.notesNeedMath = false
    if (notes.length === 0) {
      this.section = null
    } else {
      const sig = JSON.stringify([notes, linksSig, this.mathVersion, this.macroVersion])
      if (this.section === null || this.section.sig !== sig) {
        // Footnotes are lexed separately; a formula found only here must also load KaTeX.
        const noteMath = { ...math, renders: { count: 0 } }
        const html = footnotesHtml(notes, this.prefix, links, noteMath)
        math.renders.count += noteMath.renders.count
        this.section = { sig, output: { key: 'footnotes', kind: 'html', html } }
        stats.renderedBlocks += 1
      }
      this.notesNeedMath = this.section.output.kind === 'html' && this.section.output.html.includes('class="math-src"')
    }

    const blocks: RenderedBlock[] = []
    for (const entry of all) {
      if (entry.output !== null) blocks.push(entry.output)
    }
    if (this.section !== null) blocks.push(this.section.output)

    this.entries = all
    this.linksSig = linksSig
    this.lastSource = text
    return blocks
  }

  private reset(): void {
    this.frozen = []
    this.frozenEnd = 0
    this.frozenLinks = Object.create(null) as LinkMap
    this.frozenIds = new Set<string>()
  }

  /** Re-lexes a frozen block with the current definitions. Returns true when it has headings (their ids may move). */
  private relex(entry: Entry, links: LinkMap, stats: RenderStats): boolean {
    const { tokens } = lexBlocks(entry.raw, links)
    const nodes = tokens.filter((token) => token.type !== 'space')
    const node = nodes[0] as Tok | undefined
    if (nodes.length !== 1 || node === undefined || node.type !== entry.token?.type) {
      throw new Error('markdown: a frozen block does not lex alone')
    }
    stats.lexedChars += entry.raw.length
    const hadHeadings = entry.headingTokens.length > 0
    const fresh = makeEntry(entry.start, entry.raw, nodes[0] ?? null)
    entry.token = fresh.token
    entry.labels = fresh.labels
    entry.headingTokens = fresh.headingTokens
    entry.refs = fresh.refs
    entry.mathCount = fresh.mathCount
    entry.labelSig = labelSignature(entry.labels, links)
    entry.headings = []
    entry.idsSig = ''
    return hadHeadings || fresh.headingTokens.length > 0
  }

  /** Heading ids of an entry, after the ids taken so far (`taken` grows). */
  private assignIds(entry: Entry, taken: Set<string>): void {
    entry.headings = []
    const ids: string[] = []
    for (const node of entry.headingTokens) {
      const text = plainText(node.tokens as Token[] | undefined)
      const id = uniqueId(`${this.prefix}-${slugify(text)}`, taken)
      taken.add(id)
      node.bricksId = id
      ids.push(id)
      entry.headings.push({ id, text, level: node.depth as number })
    }
    entry.idsSig = ids.join('\u0002')
  }

  private renderSource(entry: Entry, previousByKey: Map<string, Entry>): void {
    const old = previousByKey.get(entry.key)
    if (entry.output !== null && entry.outputSig === entry.raw) return
    if (old?.output && old.raw === entry.raw && old.output.kind === entry.kind) {
      entry.output = old.output
      entry.outputSig = entry.raw
      return
    }
    const node = entry.token as Tok
    const source = node.text as string
    entry.output =
      entry.kind === 'mermaid'
        ? { key: entry.key, kind: 'mermaid', source, complete: entry.complete }
        : { key: entry.key, kind: 'diff', source, complete: entry.complete }
    entry.outputSig = entry.raw
  }

  private renderEntry(entry: Entry, previousByKey: Map<string, Entry>, math: MathSettings, stats: RenderStats, live: boolean): void {
    if (live && isOpenFence(entry)) {
      this.renderOpen(entry, stats)
      return
    }
    const sig = [entry.labelSig, entry.idsSig, entry.fnSig, entry.mathCount > 0 ? `m${this.mathVersion}:${this.macroVersion}` : ''].join('\u0004')
    if (entry.output !== null && entry.outputSig === sig) {
      this.restoreMacros(entry.macros)
      stats.reusedBlocks += 1
      return
    }
    const old = previousByKey.get(entry.key)
    if (old && old.raw === entry.raw) {
      if (old.output !== null && old.outputSig === sig) {
        entry.output = old.output
        entry.outputSig = sig
        entry.macros = old.macros
        this.restoreMacros(entry.macros)
        stats.reusedBlocks += 1
        return
      }
      if (old.outputSig !== '') stats.invalidatedBlocks += 1
    }
    const cached = this.cache.get(entry.key)
    if (cached && cached.raw === entry.raw && cached.sig === sig) {
      this.cache.delete(entry.key)
      this.cache.set(entry.key, cached)
      entry.output = { key: entry.key, kind: 'html', html: cached.html }
      entry.outputSig = sig
      entry.macros = cached.macros
      this.restoreMacros(entry.macros)
      stats.reusedBlocks += 1
      return
    }
    const rendered = withScope(this.prefix, () => marked.parser([entry.token as Token]))
    const beforeMacros = entry.mathCount > 0 ? { ...this.macros } : null
    const html = finishHtml(rendered.html, rendered.maths, math)
    entry.macros = null
    if (beforeMacros && (Object.keys(beforeMacros).length !== Object.keys(this.macros).length ||
      Object.keys(this.macros).some((name) => beforeMacros[name] !== this.macros[name]))) {
      entry.macros = { version: ++this.macroSerial, values: { ...this.macros } }
      this.macroVersion = entry.macros.version
    }
    this.remember(entry.key, entry.raw, sig, html, entry.macros)
    stats.renderedBlocks += 1
    entry.output = { key: entry.key, kind: 'html', html }
    entry.outputSig = sig
  }

  /**
   * An open code block while streaming: its highlighted body is kept, the text received since is
   * appended escaped, and the whole block is highlighted again once that tail is long. Nothing
   * here is cached: each tick replaces the open block. The output is safe by construction (the
   * body is highlight.js output or escaped text, the frame is ours), so it is not sanitized.
   */
  private renderOpen(entry: Entry, stats: RenderStats): void {
    const node = entry.token as Tok
    const text = node.text as string
    const language = knownLanguage(node.lang as string | undefined)
    const state = this.openCode
    let body: string
    if (state !== null && state.key === entry.key && state.language === language && text.startsWith(state.text)) {
      const tail = text.slice(state.text.length)
      if (tail.length <= PROGRESSIVE_CHARS && countNewlines(tail) <= PROGRESSIVE_LINES) {
        body = state.body + escapeHtml(tail)
      } else {
        body = highlightedBody(text, language)
        this.openCode = { key: entry.key, language, text, body }
      }
    } else {
      body = highlightedBody(text, language)
      this.openCode = { key: entry.key, language, text, body }
    }
    const info = node.lang as string | undefined
    entry.output = {
      key: entry.key,
      kind: 'html',
      html: withCodeIcons(codeShell(fenceLanguage(info) || 'texte', body, fenceIconId(info))),
    }
    entry.outputSig = OPEN_SIG
    stats.renderedBlocks += 1
  }

  /** Stores the rendering of a block under its key, replacing the previous one; evicts the least recently used within the budget. */
  private clearMacros(): void {
    for (const name of Object.keys(this.macros)) delete this.macros[name]
    this.macroVersion = 0
  }

  private restoreMacros(state: MacroState | null): void {
    if (!state) return
    this.clearMacros()
    Object.assign(this.macros, state.values)
    this.macroVersion = state.version
  }

  private remember(key: string, raw: string, sig: string, html: string, macros: MacroState | null): void {
    const previous = this.cache.get(key)
    if (previous) {
      this.cacheBytes -= previous.bytes
      this.cache.delete(key)
    }
    const bytes = 2 * (raw.length + html.length)
    this.cache.set(key, { raw, sig, html, bytes, macros })
    this.cacheBytes += bytes
    while (this.cache.size > this.cacheLimit || this.cacheBytes > CACHE_BYTES) {
      const oldest = this.cache.keys().next()
      if (oldest.done) break
      this.cacheBytes -= this.cache.get(oldest.value)?.bytes ?? 0
      this.cache.delete(oldest.value)
    }
  }
}

function countNewlines(text: string): number {
  let count = 0
  for (let index = text.indexOf('\n'); index >= 0; index = text.indexOf('\n', index + 1)) count += 1
  return count
}

/**
 * The blocks of a whole source in one lex, rendered from scratch: the oracle that
 * `MarkdownDocument.update(source, true)` must match.
 */
export function renderReference(source: string, options: MarkdownDocumentOptions & { math?: MathRenderer | null }): RenderedBlock[] {
  const doc = new MarkdownDocument(options)
  doc.setMathRenderer(options.math ?? null)
  const blocks = doc.update(source, true)
  doc.dispose()
  return blocks
}
