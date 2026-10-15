// Markdown for assistant messages. Raw HTML in the source is shown as text
// and images are not loaded (no network requests from message content).
// Links to http(s) and mailto open with the system handler through the
// opener plugin; every anchor click is intercepted so the webview never
// navigates. Other schemes stay plain text.
import type { Action } from 'svelte/action'
import DOMPurify from 'dompurify'
import hljs from 'highlight.js/lib/common'
import { openUrl } from '@tauri-apps/plugin-opener'
import { Marked } from 'marked'

export function escapeHtml(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;')
}

/** Only these schemes are rendered as links and may be opened. */
export function isOpenableUrl(href: string): boolean {
  try {
    const { protocol } = new URL(href)
    return protocol === 'https:' || protocol === 'http:' || protocol === 'mailto:'
  } catch {
    return false
  }
}

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

export interface MarkdownClickOptions {
  /** Result of an attempt to open a link: an error message, or null on success. */
  onLinkResult?: (error: string | null) => void
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

/**
 * Delegated clicks for the rendered message: copy buttons of code blocks and
 * links. Anchors never navigate the webview; openable links go to the system.
 */
export const markdownClicks: Action<HTMLElement, MarkdownClickOptions | undefined> = (node, options) => {
  let current = options

  const anchorIn = (target: EventTarget | null): HTMLAnchorElement | null => {
    if (!(target instanceof Element)) return null
    const anchor = target.closest<HTMLAnchorElement>('a[href]')
    return anchor && node.contains(anchor) ? anchor : null
  }

  const copyCode = async (button: HTMLButtonElement) => {
    const code = button.closest('.code-block')?.querySelector('code')?.textContent ?? ''
    let label = 'Copié'
    try {
      await navigator.clipboard.writeText(code)
    } catch {
      label = 'Copie impossible'
    }
    button.textContent = label
    setTimeout(() => {
      if (button.isConnected) button.textContent = 'Copier'
    }, 1500)
  }

  const onClick = (event: MouseEvent) => {
    const anchor = anchorIn(event.target)
    if (anchor) {
      // Never let the webview navigate, whatever the link target.
      event.preventDefault()
      const href = anchor.getAttribute('href') ?? ''
      if (!isOpenableUrl(href)) return
      openUrl(href).then(
        () => current?.onLinkResult?.(null),
        (error: unknown) => current?.onLinkResult?.(errorText(error)),
      )
      return
    }
    const target = event.target instanceof Element ? event.target : null
    const button = target?.closest<HTMLButtonElement>('button[data-copy]')
    if (button && node.contains(button)) void copyCode(button)
  }

  // Middle-click and other auxiliary buttons would open a new window.
  const onAuxClick = (event: MouseEvent) => {
    if (anchorIn(event.target)) event.preventDefault()
  }

  node.addEventListener('click', onClick)
  node.addEventListener('auxclick', onAuxClick)
  return {
    update(next) {
      current = next
    },
    destroy() {
      node.removeEventListener('click', onClick)
      node.removeEventListener('auxclick', onAuxClick)
    },
  }
}
