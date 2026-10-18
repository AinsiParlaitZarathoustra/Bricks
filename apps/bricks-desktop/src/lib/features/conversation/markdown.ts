// Markdown for assistant messages. Raw HTML in the source is shown as text and images are
// not loaded (no network requests from message content). The pipeline lives in ./markdown/:
// document (blocks, incremental), render (HTML), extensions (syntax), clicks (delegation).
import { renderReference } from './markdown/document'
import type { RenderedBlock } from './markdown/types'
import { codeBlockHtml } from './markdown/render'

export { escapeHtml } from './markdown/render'
export { isOpenableUrl } from './links'
export { markdownClicks, type MarkdownClickOptions } from './markdown/clicks'
export { MarkdownDocument, renderReference } from './markdown/document'

function blockHtml(block: RenderedBlock): string {
  // Diagrams and proposed diffs are not HTML: in a one-shot rendering they read as code.
  return block.kind === 'html' ? block.html : codeBlockHtml(block.source, block.kind)
}

/** Markdown source to sanitized HTML, in one piece (compatibility: the blocks joined). */
export function renderMarkdown(source: string): string {
  if (!source) return ''
  return renderReference(source, { idPrefix: 'md' }).map(blockHtml).join('')
}
