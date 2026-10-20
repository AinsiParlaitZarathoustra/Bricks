import type { DiagramViewerSettings } from '$lib/ipc/bindings/DiagramViewerSettings'
import { sanitizeSvg } from './sanitize'

export const MAX_SVG_LENGTH = 5 * 1024 * 1024
export const SVG_CSP = "default-src 'none'; script-src 'none'; style-src 'unsafe-inline'; img-src 'none'; font-src 'none'; object-src 'none'; frame-src 'none'; connect-src 'none'; base-uri 'none'; form-action 'none'"

/** Mermaid's sandbox result is an encoded iframe. Decode it inertly; never mount that iframe. */
export function mermaidSvg(result: string): string {
  if (result.length > MAX_SVG_LENGTH * 2) return ''
  if (result.trimStart().startsWith('<svg')) return sanitizeSvg(result)
  // Do not create an iframe, even in an inert parser, before checking its URL.
  const wrapper = /^<iframe\b[^>]*\bsrc="data:text\/html;charset=UTF-8;base64,([A-Za-z0-9+/=]+)"[^>]*>[\s\S]*?<\/iframe>\s*$/i.exec(result.trim())
  if (!wrapper) return ''
  try {
    const bytes = Uint8Array.from(atob(wrapper[1]), char => char.charCodeAt(0))
    const html = new TextDecoder('utf-8', { fatal: true }).decode(bytes)
    if (html.length > MAX_SVG_LENGTH) return ''
    return sanitizeSvg(html)
  } catch { return '' }
}

/** Only sanitized SVG reaches the isolated document or export. Every link stays inert. */
/** Natural size of an SVG, from its viewBox or its absolute width and height; null when unknown. */
function naturalSize(root: Element): { width: number; height: number } | null {
  const box = (root.getAttribute('viewBox') ?? '').trim().split(/[\s,]+/).map(Number)
  if (box.length === 4 && box[2] > 0 && box[3] > 0) return { width: box[2], height: box[3] }
  const width = Number.parseFloat(root.getAttribute('width') ?? '')
  const height = Number.parseFloat(root.getAttribute('height') ?? '')
  const absolute = (value: string | null) => value !== null && !value.trim().endsWith('%')
  if (width > 0 && height > 0 && absolute(root.getAttribute('width')) && absolute(root.getAttribute('height'))) return { width, height }
  return null
}

/**
 * `transparent`: the frame shows the page behind it (the app theme, whose
 * colours the diagram already uses); otherwise a plain background of `mode`.
 */
export function svgFrame(svg: string, options: DiagramViewerSettings, mode: 'light' | 'dark', background: 'plain' | 'transparent' = 'plain') {
  if (svg.length > MAX_SVG_LENGTH) return { svg: '', document: '', size: null }
  const clean = sanitizeSvg(svg)
  const doc = new DOMParser().parseFromString(clean, 'text/html')
  const root = doc.querySelector('svg')
  if (!root) return { svg: '', document: '', size: null }
  const size = naturalSize(root)
  for (const link of root.querySelectorAll('a')) link.replaceWith(...Array.from(link.childNodes))
  if (!options.allowTooltips) {
    for (const title of root.querySelectorAll('title')) title.remove()
    for (const element of [root, ...root.querySelectorAll('[title]')]) element.removeAttribute('title')
  }
  // Agent-authored CSS must not override the viewer's interaction policy.
  const controls = /(?:-webkit-)?(?:all|user-select|animation(?:-[a-z-]+)?|transition(?:-[a-z-]+)?)\s*:[^;}]*;?/gi
  for (const element of [root, ...root.querySelectorAll('*')]) {
    const style = element.getAttribute('style')
    if (style) element.setAttribute('style', style.replace(controls, ''))
    if (element.tagName.toLowerCase() === 'style') element.textContent = (element.textContent ?? '').replace(controls, '')
  }
  const safe = root.outerHTML
  const selection = options.allowSelection ? 'text' : 'none'
  const style = `html,body{margin:0;width:100%;height:100%;overflow:hidden;color-scheme:${mode};background:${background === 'transparent' ? 'transparent' : mode === 'dark' ? '#111827' : '#fff'}}svg{display:block;width:100%!important;height:100%!important;max-width:none!important}*{user-select:${selection}!important;-webkit-user-select:${selection}!important;animation:none!important;transition:none!important}`
  return {
    svg: safe,
    size,
    document: `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="${SVG_CSP}"><meta name="referrer" content="no-referrer"><style>${style}</style></head><body>${safe}</body></html>`,
  }
}
