// A diagram as a PNG image, from its filtered SVG (`frame.svg` through
// `exportableSvg`): never from the displayed frame. The sizes and the scale are
// pure (unit-tested in tests/unit/svg-png.test.ts); the rasterising uses the
// browser's image decoder and a canvas, with nothing mounted in the interface
// and nothing loaded from the network.

/** Used when the drawing tells no size of its own. */
export const FALLBACK_SIZE = { width: 1200, height: 800 } as const
/** Pixels per SVG unit, at most. */
export const TARGET_SCALE = 2
/** No side of the canvas goes past this (WebKit's limits stay far). */
export const MAX_SIDE = 8192

export interface Size {
  width: number
  height: number
}

function valid(n: unknown): n is number {
  return typeof n === 'number' && Number.isFinite(n) && n > 0
}

/** The drawing's size when it is a real one, else the fallback. */
export function pngSize(size: Size | null | undefined): Size {
  if (size && valid(size.width) && valid(size.height)) return { width: size.width, height: size.height }
  return { ...FALLBACK_SIZE }
}

/** The scale: `TARGET_SCALE`, lowered so that no side of the canvas exceeds `MAX_SIDE`. */
export function pngScale(size: Size, target = TARGET_SCALE, max = MAX_SIDE): number {
  const longest = Math.max(size.width, size.height)
  return Math.min(target, max / longest)
}

/** The canvas: whole pixels, at least one, never past `MAX_SIDE`. */
export function canvasSize(size: Size, scale: number, max = MAX_SIDE): Size {
  const side = (n: number) => Math.min(max, Math.max(1, Math.round(n * scale)))
  return { width: side(size.width), height: side(size.height) }
}

/**
 * The SVG on its own: explicit `width` and `height` (an image without them is
 * drawn at an arbitrary size), a `viewBox` when it had none, and its namespace.
 * Its fonts and styles stay as they are. `null` when there is no `<svg>` root.
 */
export function standaloneSvg(svg: string, size: Size, fontCss = ''): string | null {
  const root = svgRoot(svg)
  if (!root) return null
  const doc = root.ownerDocument
  if (fontCss) {
    const style = doc.createElementNS('http://www.w3.org/2000/svg', 'style')
    style.textContent = fontCss
    root.insertBefore(style, root.firstChild)
  }
  if (!root.getAttribute('viewBox')) root.setAttribute('viewBox', `0 0 ${size.width} ${size.height}`)
  root.setAttribute('width', String(size.width))
  root.setAttribute('height', String(size.height))
  // Its namespace comes with it: the serializer writes it.
  // Percentages and `max-width` would size the image against nothing.
  root.removeAttribute('style')
  return new XMLSerializer().serializeToString(root)
}

const GENERIC_FAMILIES = new Set([
  'serif', 'sans-serif', 'monospace', 'cursive', 'fantasy', 'system-ui', 'ui-sans-serif',
  'ui-serif', 'ui-monospace', 'ui-rounded', 'emoji', 'math', 'fangsong', 'inherit', 'initial',
])

/** The font families a drawing names (generic ones aside), lower case. */
export function fontFamilies(svg: string): string[] {
  const found = new Set<string>()
  for (const m of svg.matchAll(/font-family\s*[:=]\s*("[^"]*"|'[^']*'|[^;"'>}]+)/gi)) {
    const value = m[1].replace(/^["']|["']$/g, '').replace(/&quot;/g, '"')
    for (const part of value.split(',')) {
      const name = part.trim().replace(/^["']|["']$/g, '').trim().toLowerCase()
      if (name && !GENERIC_FAMILIES.has(name)) found.add(name)
    }
  }
  return [...found]
}

function dataUrl(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(String(reader.result))
    reader.onerror = () => reject(reader.error)
    reader.readAsDataURL(blob)
  })
}

/**
 * The app's own `@font-face` rules for the families the drawing names, with
 * their files inlined: an SVG drawn as an image sees no font the page loaded.
 * Only files of the app itself are read (same origin); a family the app does
 * not ship falls back to the system's fonts.
 */
async function embeddedFontCss(svg: string): Promise<string> {
  const wanted = new Set(fontFamilies(svg))
  if (wanted.size === 0 || typeof document === 'undefined') return ''
  const rules: string[] = []
  for (const sheet of Array.from(document.styleSheets)) {
    let list: CSSRuleList
    try {
      list = sheet.cssRules
    } catch {
      continue
    }
    for (const rule of Array.from(list)) {
      if (!(rule instanceof CSSFontFaceRule)) continue
      const family = rule.style.getPropertyValue('font-family').trim().replace(/^["']|["']$/g, '')
      if (!wanted.has(family.toLowerCase())) continue
      const src = /url\(\s*["']?([^"')]+)["']?\s*\)/.exec(rule.style.getPropertyValue('src'))?.[1]
      if (!src) continue
      const url = new URL(src, sheet.href ?? location.href)
      if (url.origin !== location.origin && url.protocol !== 'data:') continue
      try {
        const file = await (await fetch(url)).blob()
        const weight = rule.style.getPropertyValue('font-weight') || 'normal'
        const style = rule.style.getPropertyValue('font-style') || 'normal'
        rules.push(
          `@font-face{font-family:"${family}";src:url(${await dataUrl(file)});font-weight:${weight};font-style:${style}}`,
        )
      } catch {
        // That face falls back to the system's.
      }
    }
  }
  return rules.join('')
}

/**
 * The `<svg>` root of a drawing. The filtered SVG is serialized as HTML
 * (`&nbsp;` and the like are no XML): read as XML first, else as HTML, whose
 * `<svg>` serializes back as XML.
 */
function svgRoot(svg: string): Element | null {
  const SVG_NS = 'http://www.w3.org/2000/svg'
  const xml = new DOMParser().parseFromString(svg, 'image/svg+xml')
  const top = xml.documentElement
  if (!xml.querySelector('parsererror') && top?.localName === 'svg' && top.namespaceURI === SVG_NS) return top
  // Only a drawing that is the whole document: an `<svg>` inside something else is not one.
  const html = new DOMParser().parseFromString(svg, 'text/html')
  const only = html.body?.children.length === 1 ? html.body.firstElementChild : null
  return only?.localName === 'svg' ? only : null
}

/** Why a PNG could not be made. */
export class PngError extends Error {}

/**
 * The PNG of `svg` (filtered and exportable), drawn whole on an opaque
 * `background` at `pngScale`. The temporary URL is always revoked.
 */
export async function svgToPng(svg: string, drawing: Size | null, background: string): Promise<Blob> {
  const size = pngSize(drawing)
  const source = standaloneSvg(svg, size, await embeddedFontCss(svg))
  if (!source) throw new PngError('no drawing to copy')
  const scale = pngScale(size)
  const canvasDims = canvasSize(size, scale)
  const url = URL.createObjectURL(new Blob([source], { type: 'image/svg+xml;charset=utf-8' }))
  try {
    const image = new Image()
    image.decoding = 'async'
    image.src = url
    await image.decode()
    const canvas = document.createElement('canvas')
    canvas.width = canvasDims.width
    canvas.height = canvasDims.height
    const ctx = canvas.getContext('2d')
    // A canvas too large for the memory left gives no context.
    if (!ctx || canvas.width !== canvasDims.width || canvas.height !== canvasDims.height) {
      throw new PngError('the image is too large to be drawn')
    }
    ctx.fillStyle = background
    ctx.fillRect(0, 0, canvas.width, canvas.height)
    ctx.drawImage(image, 0, 0, canvas.width, canvas.height)
    const blob = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, 'image/png'))
    if (!blob) throw new PngError('the image could not be encoded')
    return blob
  } finally {
    URL.revokeObjectURL(url)
  }
}

/**
 * The colour a diagram is seen on: the first opaque background from `el` up,
 * else `fallback`. A diagram's own theme may differ from the app's.
 */
/** A computed colour with no transparency (`rgb(…)`, or an alpha of 1). */
function opaque(colour: string): boolean {
  const alpha = /^(?:rgba|hsla|color|oklch|oklab|lab|lch)\((.*)\)$/i.exec(colour.trim())?.[1]
  if (!alpha) return true
  const parts = alpha.split(/[\s,/]+/).filter(Boolean)
  const slash = alpha.includes('/')
  const last = slash || /^rgba|^hsla/i.test(colour) ? parts.at(-1) : undefined
  if (last === undefined) return true
  const value = last.endsWith('%') ? Number.parseFloat(last) / 100 : Number.parseFloat(last)
  return !Number.isFinite(value) || value >= 1
}

export function effectiveBackground(el: Element | null, fallback: string): string {
  for (let node = el; node; node = node.parentElement) {
    const colour = getComputedStyle(node).backgroundColor
    if (colour && colour !== 'transparent' && opaque(colour)) return colour
  }
  return fallback
}
