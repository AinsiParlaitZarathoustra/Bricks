// HTML sanitizing of the assistant-message pipeline, in three profiles. The
// KaTeX and SVG profiles have an instance of their own, with hooks: widening one
// never widens another. The Markdown profile is a configuration only, with no
// hook. Every function fails closed: without a DOM, the result is empty.
//
//   sanitizeMarkdown  Markdown output: no style, no media, no form; http(s),
//                     mailto and in-page fragments only.
//   sanitizeKatex     KaTeX output (HTML, MathML and the inline SVG KaTeX draws
//                     its radicals and rules with): `style` kept, but only the
//                     declarations KaTeX 0.19 emits, with plain values; no link,
//                     no external reference.
//   sanitizeSvg       Mermaid output: references to the document itself (#id)
//                     only; the diagram CSS kept, without url() to elsewhere,
//                     @import or expression().

import DOMPurify, { type Config, type DOMPurify as Purifier } from 'dompurify'

/** A DOMPurify instance of its own for a profile with hooks; null without a DOM. */
function purifier(): Purifier | null {
  if (typeof window === 'undefined') return null
  const instance = DOMPurify(window)
  return instance.isSupported ? instance : null
}

function run(instance: Purifier | null, html: string, config: Config): string {
  if (!instance || !html) return ''
  return instance.sanitize(html, config)
}

// ─── Markdown ────────────────────────────────────────────────────────────────

const MARKDOWN_CONFIG: Config = {
  // HTML only: the default of DOMPurify would also allow SVG and MathML, which Markdown never produces.
  USE_PROFILES: { html: true },
  // `type` is read as a URI otherwise, and refused: it is kept below, on buttons only.
  ADD_ATTR: ['type'],
  ADD_URI_SAFE_ATTR: ['type'],
  FORBID_TAGS: ['style', 'img', 'form', 'input', 'select', 'textarea', 'iframe', 'object', 'embed', 'link', 'meta', 'base'],
  FORBID_ATTR: ['style'],
  // Fragments of the same page (heading and footnote ids) are the only relative links.
  ALLOWED_URI_REGEXP: /^(?:https?:|mailto:|#)/i,
  ALLOW_DATA_ATTR: true,
}

/** Markdown output (marked) to safe HTML. The `type` of a button is kept only as `button`. */
export function sanitizeMarkdown(html: string): string {
  if (!html || !DOMPurify.isSupported) return ''
  const root = DOMPurify.sanitize(html, { ...MARKDOWN_CONFIG, RETURN_DOM: true }) as HTMLElement
  for (const element of root.querySelectorAll('[type]')) {
    const isButtonType =
      element.nodeName.toLowerCase() === 'button' && element.getAttribute('type') === 'button'
    if (!isButtonType) element.removeAttribute('type')
  }
  return root.innerHTML
}

// ─── CSS shared by KaTeX and Mermaid ─────────────────────────────────────────

/** `url(#id)`: a reference to this document. */
const INTERNAL_URL = /url\(\s*(['"]?)#[A-Za-z0-9_.:-]*\1\s*\)/gi
const INTERNAL_HREF = /^\s*#[A-Za-z0-9_.:-]*\s*$/
/** A link out of a diagram: a web page or a mail address, nothing that runs or loads. */
const EXTERNAL_HREF = /^\s*(?:https?:\/\/|mailto:)[^\s<>"]*\s*$/i

/** An attribute value that must not reach the DOM (outside a `style` attribute). */
function unsafeAttributeValue(value: string): boolean {
  // A backslash can spell `url(` through CSS escapes: refused as a whole.
  if (/\\|@import|expression\s*\(|javascript:|vbscript:|data:/i.test(value)) return true
  return /url\s*\(|image(?:-set)?\s*\(/i.test(value.replace(INTERNAL_URL, ''))
}

/** CSS text with every reference to another resource neutralized (`url()` to this document kept). */
export function neutralizeCss(css: string): string {
  return css
    .replace(/\\/g, '')
    .replace(/[<&]/g, '')
    .replace(/@import\b[^;]*;?/gi, '')
    .replace(/expression\s*\([^)]*\)?/gi, 'none')
    .replace(/(?:-webkit-|-moz-)?image(?:-set)?\s*\([^)]*\)?/gi, 'none')
    .replace(/-moz-binding|behavior\s*:/gi, 'none')
    .replace(/url\s*\((?!\s*['"]?#)[^)]*\)?/gi, 'none')
    .replace(/javascript:|vbscript:/gi, '')
}

// ─── KaTeX ───────────────────────────────────────────────────────────────────

/** The CSS properties KaTeX 0.19 sets inline (source and rendered corpus). */
const KATEX_PROPERTIES = new Set([
  'height',
  'width',
  'min-width',
  'vertical-align',
  'top',
  'bottom',
  'left',
  'position',
  'margin',
  'margin-top',
  'margin-left',
  'margin-right',
  'padding-left',
  'border',
  'border-width',
  'border-style',
  'border-color',
  'border-top-width',
  'border-right-width',
  'border-bottom-width',
  'border-right-style',
  'color',
  'background-color',
  'text-shadow',
])

/** One plain CSS value: a length, a keyword or a hex colour. No parenthesis, no quote, no escape. */
const KATEX_VALUE = /^(?:-?(?:\d+\.?\d*|\.\d+)(?:e-?\d+)?(?:em|ex|px|pt|%)?|[a-z]+|#[0-9a-f]{3,8})$/i

/** The declarations of a KaTeX `style` attribute that KaTeX itself could have written. */
export function katexStyle(style: string): string {
  const kept: string[] = []
  for (const declaration of style.split(';')) {
    const colon = declaration.indexOf(':')
    if (colon < 0) continue
    const property = declaration.slice(0, colon).trim().toLowerCase()
    const value = declaration.slice(colon + 1).trim()
    if (!KATEX_PROPERTIES.has(property)) continue
    // KaTeX only positions its boxes relatively: `fixed` or `absolute` would escape the message.
    if (property === 'position' && value !== 'relative') continue
    if (!value.split(/\s+/).every((token) => KATEX_VALUE.test(token))) continue
    kept.push(`${property}: ${value}`)
  }
  return kept.join('; ')
}

const katexPurifier = purifier()

katexPurifier?.addHook('uponSanitizeAttribute', (_node, data) => {
  if (data.attrName !== 'style') return
  data.attrValue = katexStyle(data.attrValue)
  if (data.attrValue === '') data.keepAttr = false
})

const KATEX_CONFIG: Config = {
  USE_PROFILES: { html: true, mathMl: true, svg: true },
  ADD_ATTR: [
    'style',
    'display',
    'displaystyle',
    'scriptlevel',
    'fence',
    'stretchy',
    'mathvariant',
    'mathcolor',
    'mathbackground',
    'accent',
    'minsize',
    'lspace',
    'rspace',
    'voffset',
    'rowspacing',
    'columnspacing',
    'columnalign',
    'columnlines',
    'notation',
    'depth',
    'encoding',
    'preserveAspectRatio',
  ],
  // KaTeX draws with inline shapes only: no link, no external resource, no animation.
  FORBID_TAGS: [
    'a',
    'image',
    'use',
    'foreignobject',
    'script',
    'style',
    'iframe',
    'object',
    'embed',
    'form',
    'input',
    'img',
    'link',
    'meta',
    'base',
    'animate',
    'set',
    'animatemotion',
    'animatetransform',
  ],
  FORBID_ATTR: ['href', 'xlink:href', 'src', 'srcset', 'action', 'formaction'],
  ALLOW_DATA_ATTR: false,
}

/** KaTeX output (HTML, MathML, inline SVG) to safe HTML; `style` filtered by `katexStyle`. */
export function sanitizeKatex(html: string): string {
  return run(katexPurifier, html, KATEX_CONFIG)
}

// ─── SVG (Mermaid) ───────────────────────────────────────────────────────────

const svgPurifier = purifier()

svgPurifier?.addHook('uponSanitizeAttribute', (node, data) => {
  if (data.attrName === 'style') {
    data.attrValue = neutralizeCss(data.attrValue)
    if (data.attrValue.trim() === '') data.keepAttr = false
    return
  }
  if (data.attrName === 'href' || data.attrName === 'xlink:href') {
    // An anchor may lead out (the viewer decides whether it is followed); anything else stays internal.
    const external = node.nodeName.toLowerCase() === 'a' && EXTERNAL_HREF.test(data.attrValue)
    if (!external && !INTERNAL_HREF.test(data.attrValue)) data.keepAttr = false
    return
  }
  if (unsafeAttributeValue(data.attrValue)) data.keepAttr = false
})

const SVG_CONFIG: Config = {
  USE_PROFILES: { svg: true, svgFilters: true },
  FORBID_TAGS: [
    'foreignobject',
    'script',
    'image',
    'feimage',
    'use',
    'iframe',
    'object',
    'embed',
    'animate',
    'set',
    'animatemotion',
    'animatetransform',
    'discard',
  ],
}

/**
 * Mermaid's SVG to safe SVG: markers, defs and ids kept; no foreign content, no external reference.
 * The diagram's own `<style>` is outside DOMPurify (it drops `style` inside SVG): its CSS is
 * cleaned by `cssSafe` and put back, after sanitizing, as the first child of the root element.
 */
export function sanitizeSvg(svg: string): string {
  if (!svgPurifier || !svg) return ''
  // Template contents stay inert: no scripts, frame loads or image fetches while inspecting SVG.
  const parsed = document.createElement('template')
  parsed.innerHTML = svg
  const css: string[] = []
  for (const style of parsed.content.querySelectorAll('style')) {
    css.push(neutralizeCss(style.textContent ?? ''))
    style.remove()
  }
  const body = run(svgPurifier, parsed.innerHTML, SVG_CONFIG)
  const diagramCss = css.join('\n').trim()
  if (diagramCss === '' || !/^<svg\b/i.test(body)) return body
  return body.replace(/^<svg\b[^>]*>/i, (open) => `${open}<style>${diagramCss}</style>`)
}
