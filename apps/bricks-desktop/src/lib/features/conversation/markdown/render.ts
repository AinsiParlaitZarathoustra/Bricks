// HTML of the assistant Markdown. One marked instance, configured once here. Block HTML
// is raw until `finishHtml`: sanitized (sanitizeMarkdown), then math placeholders replaced
// by the math renderer (or by the source when none is installed).
import hljs from 'highlight.js/lib/common'
import { Lexer, Marked, type Parser, type Token, type Tokens } from 'marked'
import { isOpenableUrl } from '../links'
import { parseFileReference, safeDecode } from './fileref-parse'
import {
  alertBlockquote,
  DUPLICATE_MARK,
  linkDefinition,
  blockMathExtension,
  fenceLanguage,
  footnoteDefExtension,
  footnoteRefExtension,
  inlineMathExtension,
  refineTokens,
  slugify,
  type Tok,
} from './extensions'
import { sanitizeMarkdown } from './sanitize'
import type { FileReference, MathRenderer } from './types'

/** A math expression of the block being rendered, replaced after sanitizing. */
export interface MathItem {
  tex: string
  display: boolean
  open: string
  close: string
}

/** Link definitions known to the lexer: normalized label → target. */
export type LinkMap = Record<string, { href: string; title?: string | null }>

/** Math settings of one render: the renderer (null until KaTeX is loaded), the per-message macros and a call counter. */
export interface MathSettings {
  renderer: MathRenderer | null
  macros: Record<string, string>
  renders: { count: number }
}

interface Scope {
  prefix: string
  maths: MathItem[]
}

/** The message being rendered (set around marked's parser, never shared between renders). */
let scope: Scope | null = null
let highlights = 0

/** Number of highlight.js calls so far; callers take differences. */
export function highlightCount(): number {
  return highlights
}

export function escapeHtml(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;')
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

const LINK_CLASS =
  'rounded-sm text-primary underline underline-offset-2 hover:opacity-80 focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none'

const ALERT_TITLES: Record<string, string> = {
  note: 'Note',
  tip: 'Astuce',
  important: 'Important',
  warning: 'Attention',
  caution: 'Danger',
}

/** The language highlight.js knows for a fence, or '' for plain text. */
export function knownLanguage(lang: string | undefined): string {
  const language = fenceLanguage(lang)
  return language !== '' && hljs.getLanguage(language) !== undefined ? language : ''
}

/** The body of a code block: highlighted when `language` is not '', escaped otherwise. Always safe HTML. */
export function highlightedBody(text: string, language: string): string {
  if (language === '') return escapeHtml(text)
  highlights += 1
  return hljs.highlight(text, { language, ignoreIllegals: true }).value
}

/** The frame of a code block: its label, its copy button and its body (safe HTML). */
export function codeShell(label: string, body: string): string {
  return (
    `<div class="code-block my-2 overflow-hidden rounded-md border border-border bg-code-bg text-code-foreground">` +
    `<div class="flex items-center justify-between gap-2 border-b border-border px-2.5 py-1 text-xs text-muted-foreground">` +
    `<span class="font-mono">${escapeHtml(label)}</span>` +
    `<button type="button" data-copy class="rounded px-1.5 py-0.5 hover:bg-muted hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none">Copier</button>` +
    `</div>` +
    `<pre class="overflow-x-auto p-2.5 text-xs leading-relaxed"><code class="hljs font-mono">${body}</code></pre>` +
    `</div>`
  )
}

/** A code block as HTML: highlighted when the language is known, escaped otherwise. */
export function codeBlockHtml(text: string, lang: string): string {
  return codeShell(fenceLanguage(lang) || 'texte', highlightedBody(text, knownLanguage(lang)))
}

function fileButton(ref: FileReference, label: string): string {
  const line = ref.line === undefined ? '' : ` data-line="${ref.line}"`
  const endLine = ref.endLine === undefined ? '' : ` data-end-line="${ref.endLine}"`
  return (
    `<button type="button" class="file-ref" data-file-path="${escapeHtml(ref.path)}"${line}${endLine}>` +
    `${escapeHtml(label)}</button>`
  )
}

function mathPlaceholder(token: Tok): string {
  if (!scope) return escapeHtml(token.raw)
  scope.maths.push({ tex: token.text as string, display: token.display === true, open: token.open as string, close: token.close as string })
  return `<span data-math="${scope.maths.length - 1}"></span>`
}

// Footnote ids are `P-fn:ID` (note) and `P-fnref:ID-K` (reference K). Heading ids and link
// fragments are `P-` followed by a slug, and a slug only holds letters, digits and dashes: no
// colon, so no footnote id can be a heading id. `fn:` and `fnref:` differ at their third
// character, and `K` is numeric (no dash), so every footnote id is unique.
function footnoteRefHtml(token: Tok): string {
  if (typeof token.number !== 'number') return escapeHtml(token.raw)
  const prefix = scope?.prefix ?? ''
  const id = escapeHtml(token.id as string)
  return (
    `<sup class="fn-ref"><a href="#${prefix}-fn:${id}" id="${prefix}-fnref:${id}-${token.occurrence}" data-fn>` +
    `${token.number}</a></sup>`
  )
}

function alertHtml(variant: string, body: string): string {
  return (
    `<div class="md-alert md-alert-${variant}" role="note">` +
    `<p class="md-alert-title" data-alert-icon="${variant}">${ALERT_TITLES[variant] ?? variant}</p>` +
    `${body}</div>`
  )
}

const renderer = {
  code({ text, lang }: Tokens.Code) {
    return codeBlockHtml(text, lang ?? '')
  },
  heading(this: { parser: Parser }, token: Tokens.Heading) {
    // Message headings start one level below the page title: never a second h1.
    const level = Math.min(token.depth + 1, 6)
    const id = (token as Tok).bricksId as string | undefined
    const attribute = id ? ` id="${escapeHtml(id)}"` : ''
    return `<h${level}${attribute}>${this.parser.parseInline(token.tokens)}</h${level}>\n`
  },
  html({ text }: Tokens.HTML | Tokens.Tag) {
    return escapeHtml(text)
  },
  link({ href, text }: Tokens.Link) {
    const label = escapeHtml(text || href)
    if (href.startsWith('#')) {
      // An in-page anchor: the id a heading or footnote of this message gets.
      const fragment = safeDecode(href.slice(1)) ?? href.slice(1)
      const target = `#${scope?.prefix ?? ''}-${slugify(fragment)}`
      return `<a href="${escapeHtml(target)}" class="${LINK_CLASS}" data-local-anchor>${label}</a>`
    }
    const file = parseFileReference(href, 'href')
    if (file) return fileButton(file, text || href)
    if (!isOpenableUrl(href)) return label
    const address = escapeHtml(href)
    // The address is shown when the link text differs, so the destination is visible.
    const shownAddress = text && text !== href ? ` <span class="text-muted-foreground">(${address})</span>` : ''
    return (
      `<a href="${address}" rel="noopener noreferrer nofollow" title="${address}" class="${LINK_CLASS}">` +
      `${label}</a>${shownAddress}`
    )
  },
  image({ text, href }: Tokens.Image) {
    return `<span class="text-muted-foreground">[image : ${escapeHtml(text || href || 'sans titre')}]</span>`
  },
}

/** The single marked instance of the assistant Markdown. */
export const marked = new Marked({
  gfm: true,
  breaks: true,
  extensions: [
    inlineMathExtension,
    blockMathExtension,
    footnoteRefExtension,
    footnoteDefExtension,
    {
      name: 'inlineMath',
      renderer: (token) => mathPlaceholder(token as Tok),
    },
    {
      name: 'blockMath',
      renderer: (token) => mathPlaceholder(token as Tok),
    },
    {
      name: 'footnoteRef',
      renderer: (token) => footnoteRefHtml(token as Tok),
    },
    {
      name: 'footnoteDef',
      renderer: () => '',
    },
    {
      name: 'fileRef',
      renderer: (token) => {
        const node = token as Tok
        return fileButton(node.ref as FileReference, node.text as string)
      },
    },
    {
      name: 'alert',
      renderer(token) {
        const node = token as Tok
        return alertHtml(node.variant as string, this.parser.parse((node.tokens as Token[] | undefined) ?? []))
      },
    },
  ],
  tokenizer: { blockquote: alertBlockquote, def: linkDefinition },
  renderer,
})

/** Link definitions of a source, normalized labels (what marked stores in `tokens.links`). */
function newLexer(links: LinkMap): Lexer {
  const lexer = new Lexer(marked.defaults)
  Object.assign(lexer.tokens.links, links)
  return lexer
}

/** Block tokens of a source (refined), and the link definitions known after the lex. */
export function lexBlocks(src: string, links: LinkMap): { tokens: Token[]; links: LinkMap } {
  const lexer = newLexer(links)
  const tokens = lexer.lex(src)
  // The internal keys of repeated definitions are not links.
  const known: LinkMap = Object.create(null) as LinkMap
  for (const label of Object.keys(lexer.tokens.links)) {
    if (!label.includes(DUPLICATE_MARK)) known[label] = lexer.tokens.links[label] as LinkMap[string]
  }
  return { tokens: refineTokens(tokens), links: known }
}

/** Inline tokens of a text (footnote contents), with the given link definitions. */
export function lexInline(text: string, links: LinkMap): Token[] {
  return refineTokens(newLexer(links).inlineTokens(text))
}

/** Runs `run` with a message scope: math placeholders and footnote ids of this message only. */
export function withScope(prefix: string, run: () => string): { html: string; maths: MathItem[] } {
  const previous = scope
  const current: Scope = { prefix, maths: [] }
  scope = current
  try {
    return { html: run(), maths: current.maths }
  } finally {
    scope = previous
  }
}

/** Sanitizes block HTML, then replaces each math placeholder by the renderer output or by its source. */
export function finishHtml(raw: string, maths: MathItem[], math: MathSettings): string {
  const html = sanitizeMarkdown(raw)
  if (maths.length === 0) return html
  return html.replace(/<span data-math="(\d+)"><\/span>/g, (_match, index: string) =>
    mathHtml(maths[Number(index)] as MathItem, math),
  )
}

function mathHtml(item: MathItem, math: MathSettings): string {
  const source = escapeHtml(item.open + item.tex + item.close)
  if (!math.renderer) return `<span class="math-src" data-display="${item.display ? 1 : 0}">${source}</span>`
  math.renders.count += 1
  let result: { html: string } | { error: string }
  try {
    result = math.renderer(item.tex, item.display, math.macros)
  } catch (error) {
    result = { error: errorText(error) }
  }
  if ('error' in result) return `<span class="math-error" title="${escapeHtml(result.error)}">${source}</span>`
  return result.html
}

export interface FootnoteNote {
  id: string
  text: string
  /** Number of references, for the back links. */
  count: number
}

/** The footnote section: notes in order of first reference, each with a back link per reference. */
export function footnotesHtml(notes: FootnoteNote[], prefix: string, links: LinkMap, math: MathSettings): string {
  const { html, maths } = withScope(prefix, () => {
    const items = notes.map((note) => {
      const paragraph = marked.parser([
        { type: 'paragraph', raw: note.text, text: note.text, tokens: lexInline(note.text, links) } as unknown as Token,
      ])
      const backs = Array.from(
        { length: note.count },
        (_unused, index) =>
          `<a href="#${prefix}-fnref:${escapeHtml(note.id)}-${index + 1}" data-fn-back aria-label="Retour au texte">↩</a>`,
      ).join(' ')
      const withBacks = paragraph.replace(/<\/p>\n?$/, ` ${backs}</p>`)
      return `<li id="${prefix}-fn:${escapeHtml(note.id)}">${withBacks}</li>`
    })
    return `<section class="md-footnotes" aria-label="Notes"><ol>${items.join('')}</ol></section>`
  })
  return finishHtml(html, maths, math)
}
