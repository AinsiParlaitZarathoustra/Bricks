// FROZEN BASELINE: do not modify.
// Provenance: `git show c1fe3de:apps/bricks-desktop/src/lib/features/conversation/markdown.ts`
// (commit c1fe3de, "chore: update Bricks app icons and favicon"), kept for the
// Sprint 14.5 benchmark (tests/bench/run-baseline.mjs). Adjustments only:
// the opener import and the Svelte click action were removed (not measured).
// `renderMarkdown` and its configuration are unchanged.
//
// Markdown for assistant messages. Raw HTML in the source is shown as text
// and images are not loaded (no network requests from message content).

import DOMPurify from 'dompurify'
import hljs from 'highlight.js/lib/common'
import { Marked } from 'marked'
import { isOpenableUrl } from './links.ts'

export function escapeHtml(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;')
}

export { isOpenableUrl } from './links.ts'

const marked = new Marked({
  gfm: true,
  breaks: true,
  renderer: {
    code({ text, lang }) {
      const language = (lang ?? '').trim().split(/\s+/)[0]?.toLowerCase() ?? ''
      const known = language !== '' && hljs.getLanguage(language) !== undefined
      const body = known
        ? hljs.highlight(text, { language, ignoreIllegals: true }).value
        : escapeHtml(text)
      const label = known ? language : language || 'texte'
      return (
        `<div class="code-block my-2 overflow-hidden rounded-md border border-border bg-code-bg text-code-foreground">` +
        `<div class="flex items-center justify-between gap-2 border-b border-border px-2.5 py-1 text-xs text-muted-foreground">` +
        `<span class="font-mono">${escapeHtml(label)}</span>` +
        `<button type="button" data-copy class="rounded px-1.5 py-0.5 hover:bg-muted hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none">Copier</button>` +
        `</div>` +
        `<pre class="overflow-x-auto p-2.5 text-xs leading-relaxed"><code class="hljs font-mono">${body}</code></pre>` +
        `</div>`
      )
    },
    heading({ tokens, depth }) {
      // Message headings start one level below the page title: never a second h1.
      const level = Math.min(depth + 1, 6)
      return `<h${level}>${this.parser.parseInline(tokens)}</h${level}>\n`
    },
    html({ text }) {
      return escapeHtml(text)
    },
    link({ href, text }) {
      const label = escapeHtml(text || href)
      if (!isOpenableUrl(href)) return label
      const address = escapeHtml(href)
      // The address is shown when the link text differs, so the destination is visible.
      const shownAddress =
        text && text !== href ? ` <span class="text-muted-foreground">(${address})</span>` : ''
      return (
        `<a href="${address}" rel="noopener noreferrer nofollow" title="${address}" ` +
        `class="rounded-sm text-primary underline underline-offset-2 hover:opacity-80 focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none">` +
        `${label}</a>${shownAddress}`
      )
    },
    image({ text, href }) {
      return `<span class="text-muted-foreground">[image : ${escapeHtml(text || href || 'sans titre')}]</span>`
    },
  },
})

const SANITIZE = {
  FORBID_TAGS: ['style', 'img', 'form', 'input', 'select', 'textarea', 'iframe', 'object', 'embed', 'link', 'meta', 'base'],
  FORBID_ATTR: ['style'],
  ALLOWED_URI_REGEXP: /^(?:https?:|mailto:)/i,
}

/** Markdown source to sanitized HTML. */
export function renderMarkdown(source: string): string {
  if (!source) return ''
  const html = marked.parse(source, { async: false })
  return DOMPurify.sanitize(html, SANITIZE)
}
