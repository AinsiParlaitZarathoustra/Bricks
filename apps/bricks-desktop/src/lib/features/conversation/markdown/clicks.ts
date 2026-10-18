// Delegated clicks of a rendered message. Copies code, opens external links through the
// system (never while streaming), jumps to in-page anchors (headings, footnotes) inside the
// conversation, and reports file references. Nothing here executes content.
import type { Action } from 'svelte/action'
import { openUrl } from '@tauri-apps/plugin-opener'
import { isOpenableUrl } from '../links'
import type { FileReference } from './types'

export interface MarkdownClickOptions {
  /** Result of an attempt to open a link: an error message, or null on success. */
  onLinkResult?: (error: string | null) => void
  /** False while the text is still streaming: a half-received address must not open. Default true. */
  linksEnabled?: boolean
  /** A file reference (`.file-ref` button) was activated, by pointer or keyboard. */
  onFileReference?: (ref: FileReference) => void
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

/** The in-page target of an id, searched in the conversation container when there is one. */
function findTarget(node: HTMLElement, id: string): HTMLElement | null {
  const root: ParentNode = node.closest('[data-conversation-scroll]') ?? node.ownerDocument
  return root.querySelector<HTMLElement>(`[id="${id.replace(/["\\]/g, '\\$&')}"]`)
}

function jumpTo(node: HTMLElement, href: string): void {
  const target = findTarget(node, href.slice(1))
  if (!target) return
  if (!target.hasAttribute('tabindex')) target.setAttribute('tabindex', '-1')
  target.scrollIntoView?.({ block: 'start' })
  target.focus?.({ preventScroll: true })
}

function fileReferenceOf(button: HTMLButtonElement): FileReference | null {
  const path = button.dataset.filePath
  if (!path) return null
  const line = button.dataset.line === undefined ? NaN : Number(button.dataset.line)
  const endLine = button.dataset.endLine === undefined ? NaN : Number(button.dataset.endLine)
  if (!Number.isInteger(line)) return { path }
  return Number.isInteger(endLine) ? { path, line, endLine } : { path, line }
}

/**
 * Delegated clicks for a rendered message: copy buttons of code blocks, links, in-page
 * anchors and file references. Anchors never navigate the webview.
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
      if (href.startsWith('#')) {
        jumpTo(node, href)
        return
      }
      if (current?.linksEnabled === false || !isOpenableUrl(href)) return
      openUrl(href).then(
        () => current?.onLinkResult?.(null),
        (error: unknown) => current?.onLinkResult?.(errorText(error)),
      )
      return
    }
    const target = event.target instanceof Element ? event.target : null
    const file = target?.closest<HTMLButtonElement>('button.file-ref')
    if (file && node.contains(file)) {
      // Like links, file references wait for the end of the stream.
      if (current?.linksEnabled === false) return
      const ref = fileReferenceOf(file)
      if (ref) current?.onFileReference?.(ref)
      return
    }
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
