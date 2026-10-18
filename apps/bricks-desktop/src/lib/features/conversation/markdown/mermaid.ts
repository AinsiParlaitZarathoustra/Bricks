// Mermaid diagrams of assistant messages. Mermaid is loaded on demand. Bricks
// owns its configuration: a diagram cannot change the security level, the
// size bounds or the theme (they are in `secure`). Renders run one at a time,
// the source is neutralized first, and the SVG goes through `sanitizeSvg`.
//
// Bounds: MAX_SOURCE_LENGTH characters (checked before Mermaid sees the text),
// MAX_EDGES edges per diagram (Mermaid's own limit).

import { sanitizeSvg } from './sanitize'

/** Longest source given to Mermaid, in characters (Mermaid's maxTextSize). */
export const MAX_SOURCE_LENGTH = 20000
/** Most edges in one diagram (Mermaid's maxEdges). */
export const MAX_EDGES = 300

const DEFAULT_FONT = 'sans-serif'
const ID_PATTERN = /^[A-Za-z][A-Za-z0-9_-]{0,80}$/
const MAX_ERROR_LENGTH = 200

type MermaidApi = (typeof import('mermaid'))['default']
type MermaidConfig = Parameters<MermaidApi['initialize']>[0]

export interface MermaidRenderOptions {
  /** Unique id of the SVG (Mermaid's id): letters, digits, `-` and `_`. */
  id: string
  theme: 'light' | 'dark'
  fontFamily: string
  /** Colours of the application theme: background, foreground, card, border, primary, muted. */
  colors?: Record<string, string>
  /** The cache of the message (see `createMermaidCache`). Without it, nothing is cached. */
  cache?: MermaidCache
}

export type MermaidResult = { svg: string } | { error: string }

/** A bounded cache of rendered SVG, least recently used first out. One per message. */
export interface MermaidCache {
  get(key: string): string | undefined
  set(key: string, svg: string): void
}

export function createMermaidCache(limit = 32): MermaidCache {
  const entries = new Map<string, string>()
  return {
    get(key) {
      const svg = entries.get(key)
      if (svg !== undefined) {
        entries.delete(key)
        entries.set(key, svg)
      }
      return svg
    },
    set(key, svg) {
      entries.delete(key)
      if (limit < 1) return
      entries.set(key, svg)
      while (entries.size > limit) {
        const oldest = entries.keys().next().value
        if (oldest === undefined) break
        entries.delete(oldest)
      }
    },
  }
}

// ─── Source ──────────────────────────────────────────────────────────────────

/** Why a source cannot be rendered at all (size, emptiness), or `null`. */
export function mermaidSourceError(source: string): string | null {
  if (source.length > MAX_SOURCE_LENGTH) {
    return `Diagramme trop long (${MAX_SOURCE_LENGTH} caractères au maximum)`
  }
  return null
}

/** A declaration or statement that could fetch a resource or run code, in a style line. */
const UNSAFE_STYLE = /url\s*\(|@import|expression\s*\(|javascript:|\\/i
const STYLE_STATEMENT = /^\s*(?:classDef|style|linkStyle)\b/i
/**
 * `click A call …`, `click A href …`, `click A "url"`: an interaction statement, at the
 * start of a line or after a `;` separator. The rest of its line goes with it.
 */
const INTERACTION = /(^|;)\s*click\s+(?![-=.:])/i

/**
 * The source Mermaid receives: front matter, directives (`%%{ … }%%`),
 * interactions (`click`) and styles that load or run something are removed.
 */
export function neutralizeMermaid(source: string): string {
  let text = source.replace(/\r\n?/g, '\n')
  // Front matter: a block between two `---` lines at the very top.
  text = text.replace(/^\s*---\n[\s\S]*?\n---[ \t]*(?=\n|$)/, '')
  // Directives, closed or not (an unclosed one ends with its line).
  text = text.replace(/%%\{[\s\S]*?\}%%/g, '').replace(/%%\{[^\n]*/g, '')
  return text
    .split('\n')
    .map(dropInteraction)
    .filter((line) => !(STYLE_STATEMENT.test(line) && UNSAFE_STYLE.test(line)))
    .join('\n')
}

/** The line up to its first interaction statement (the statement and what follows it are dropped). */
function dropInteraction(line: string): string {
  const match = INTERACTION.exec(line)
  return match ? line.slice(0, match.index) : line
}

// ─── Theme and configuration ─────────────────────────────────────────────────

/** Keys of the palette; `chart1`…`chart5` are the series colours of charts, pies and branches. */
const PALETTE: Record<MermaidRenderOptions['theme'], Record<string, string>> = {
  light: {
    background: '#ffffff',
    foreground: '#1f2328',
    card: '#f6f8fa',
    border: '#d0d7de',
    primary: '#2f6feb',
    muted: '#57606a',
    chart1: '#a65a35',
    chart2: '#3b6db0',
    chart3: '#2f7a4d',
    chart4: '#9a6a1f',
    chart5: '#6d6a64',
  },
  dark: {
    background: '#0d1117',
    foreground: '#e6edf3',
    card: '#161b22',
    border: '#30363d',
    primary: '#4493f8',
    muted: '#8b949e',
    chart1: '#e0936b',
    chart2: '#8fb4ea',
    chart3: '#79c995',
    chart4: '#e3b562',
    chart5: '#aaa7a1',
  },
}

const CHART_KEYS = ['chart1', 'chart2', 'chart3', 'chart4', 'chart5']

const COLOR_VALUE = /^(?:#[0-9a-f]{3,8}|[a-z]{3,20}|(?:hsla?|rgba?|oklch)\([\d\s.,%/+-]{1,60}\))$/i
const FONT_VALUE = /^[\p{L}\p{N}\s\-_,.'"]{1,200}$/u

/** The colours Bricks passes on: known keys, plain CSS colours only. */
export function safeColors(colors: Record<string, string> | undefined): Record<string, string> {
  const safe: Record<string, string> = {}
  for (const key of Object.keys(PALETTE.light)) {
    const value = colors?.[key]?.trim()
    if (value && COLOR_VALUE.test(value)) safe[key] = value
  }
  return safe
}

function safeFont(fontFamily: string): string {
  const trimmed = fontFamily.trim()
  return FONT_VALUE.test(trimmed) ? trimmed : DEFAULT_FONT
}

/** `#rgb` or `#rrggbb` to its relative luminance (WCAG); null for any other colour form. */
function luminance(colour: string): number | null {
  const hex = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(colour)
  if (!hex) return null
  const digits = hex[1].length === 3 ? [...hex[1]].map((d) => d + d).join('') : hex[1]
  const linear = [0, 2, 4].map((i) => {
    const channel = Number.parseInt(digits.slice(i, i + 2), 16) / 255
    return channel <= 0.03928 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4
  })
  return 0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2]
}

function contrast(a: number, b: number): number {
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05)
}

/**
 * The text colour, among the candidates in order, that reads best on every fill:
 * the candidate whose smallest contrast with the fills is the largest (the first one
 * wins a tie). When a fill or a candidate is not a hex colour, the first candidate.
 */
export function readableText(fills: string[], candidates: string[]): string {
  const [first = ''] = candidates
  const fillLuminance = fills.map(luminance)
  if (fills.length === 0 || fillLuminance.some((value) => value === null)) return first
  let best = first
  let bestContrast = -1
  for (const candidate of candidates) {
    const text = luminance(candidate)
    if (text === null) continue
    const worst = Math.min(...fillLuminance.map((fill) => contrast(text, fill ?? 0)))
    if (worst > bestContrast) {
      best = candidate
      bestContrast = worst
    }
  }
  return best
}

/**
 * The theme variables of a render: the surfaces and texts of the application, the
 * chart colours (pies, XY charts, timelines and mindmaps, git branches) cycled from
 * the five `chart` keys, and the label colours that read on them.
 */
export function themeVariables(
  theme: MermaidRenderOptions['theme'],
  colors: Record<string, string>,
  fontFamily: string,
): Record<string, unknown> {
  const c = { ...PALETTE[theme], ...colors }
  const charts = CHART_KEYS.map((key) => c[key] ?? c.primary ?? '')
  const cycle = (index: number) => charts[index % charts.length] ?? ''
  const series: Record<string, string> = {}
  for (let i = 0; i < 12; i++) {
    series[`pie${i + 1}`] = cycle(i)
    series[`cScale${i}`] = cycle(i)
    series[`cScaleLabel${i}`] = c.background ?? ''
  }
  for (let i = 0; i < 8; i++) {
    series[`git${i}`] = cycle(i)
    series[`gitInv${i}`] = c.background ?? ''
  }
  // Section text: the candidate that reads best on every chart colour.
  const sectionText = readableText(charts, [
    c.foreground ?? '',
    c.background ?? '',
    PALETTE.light.foreground ?? '',
    PALETTE.dark.foreground ?? '',
  ])
  return {
    ...series,
    pieStrokeColor: c.border,
    pieOuterStrokeColor: c.border,
    pieTitleTextColor: c.foreground,
    pieLegendTextColor: c.foreground,
    pieSectionTextColor: sectionText,
    xyChart: {
      backgroundColor: c.background,
      titleColor: c.foreground,
      dataLabelColor: c.foreground,
      legendTextColor: c.foreground,
      xAxisTitleColor: c.foreground,
      xAxisLabelColor: c.foreground,
      xAxisTickColor: c.muted,
      xAxisLineColor: c.muted,
      yAxisTitleColor: c.foreground,
      yAxisLabelColor: c.foreground,
      yAxisTickColor: c.muted,
      yAxisLineColor: c.muted,
      plotColorPalette: charts.join(','),
    },
    fontFamily,
    background: c.background,
    textColor: c.foreground,
    primaryColor: c.card,
    primaryTextColor: c.foreground,
    primaryBorderColor: c.border,
    secondaryColor: c.card,
    tertiaryColor: c.background,
    mainBkg: c.card,
    nodeBorder: c.border,
    lineColor: c.muted,
    clusterBkg: c.card,
    clusterBorder: c.border,
    titleColor: c.foreground,
    edgeLabelBackground: c.background,
    noteBkgColor: c.card,
    noteBorderColor: c.border,
    noteTextColor: c.foreground,
    actorBkg: c.card,
    actorBorder: c.border,
    actorTextColor: c.foreground,
    signalColor: c.foreground,
    signalTextColor: c.foreground,
    labelBoxBkgColor: c.card,
    labelBoxBorderColor: c.border,
    labelTextColor: c.foreground,
    loopTextColor: c.foreground,
    activationBkgColor: c.card,
    activationBorderColor: c.border,
    sequenceNumberColor: c.background,
    accent: c.primary,
  }
}

/** The keys Bricks owns: a diagram's directive cannot override any of them. */
const SECURE_KEYS = [
  'secure',
  'securityLevel',
  'startOnLoad',
  'htmlLabels',
  'maxTextSize',
  'maxEdges',
  'suppressErrorRendering',
  'logLevel',
  'theme',
  'themeVariables',
  'fontFamily',
]

function mermaidConfig(
  theme: MermaidRenderOptions['theme'],
  colors: Record<string, string>,
  fontFamily: string,
): MermaidConfig {
  return {
    startOnLoad: false,
    securityLevel: 'strict',
    htmlLabels: false,
    maxTextSize: MAX_SOURCE_LENGTH,
    maxEdges: MAX_EDGES,
    suppressErrorRendering: true,
    logLevel: 'fatal',
    secure: SECURE_KEYS,
    theme: 'base',
    fontFamily,
    themeVariables: themeVariables(theme, colors, fontFamily),
  }
}

// ─── Loading and rendering ───────────────────────────────────────────────────

let loading: Promise<MermaidApi> | null = null
/** The theme the library was last initialized with (see `applyTheme`). */
let appliedKey: string | null = null

/** Mermaid, initialized once with Bricks' configuration. A failed load is retried later. */
export function loadMermaid(): Promise<MermaidApi> {
  loading ??= import('mermaid')
    .then(({ default: mermaid }) => {
      mermaid.initialize(mermaidConfig('light', {}, DEFAULT_FONT))
      appliedKey = JSON.stringify(['light', DEFAULT_FONT, {}])
      return mermaid
    })
    .catch((error: unknown) => {
      loading = null
      throw error
    })
  return loading
}

/**
 * Mermaid reads its theme when it is initialized, not per render: it is
 * initialized again only when the theme, the font or the colours change.
 */
function applyTheme(mermaid: MermaidApi, theme: MermaidRenderOptions['theme'], colors: Record<string, string>, fontFamily: string) {
  const key = JSON.stringify([theme, fontFamily, colors])
  if (key === appliedKey) return
  mermaid.initialize(mermaidConfig(theme, colors, fontFamily))
  appliedKey = key
}

// Mermaid keeps one shared state: renders run one after the other.
let queue: Promise<unknown> = Promise.resolve()

function serialized<T>(task: () => Promise<T>): Promise<T> {
  const run = queue.then(task)
  queue = run.then(
    () => undefined,
    () => undefined,
  )
  return run
}

function shortMessage(error: unknown): string {
  const text =
    error instanceof Error ? error.message : typeof error === 'string' ? error : String(error)
  const first = text.split('\n')[0]?.trim() ?? ''
  return (first || 'Diagramme invalide').slice(0, MAX_ERROR_LENGTH)
}

/** Mermaid leaves its working nodes in the document after a failed render; they go. */
function removeLeftovers(id: string) {
  if (typeof document === 'undefined') return
  for (const leftover of [document.getElementById(id), document.getElementById(`d${id}`)]) {
    leftover?.remove()
  }
}

async function renderOnce(source: string, options: {
  id: string
  theme: MermaidRenderOptions['theme']
  fontFamily: string
  colors: Record<string, string>
}): Promise<MermaidResult> {
  let mermaid: MermaidApi
  try {
    mermaid = await loadMermaid()
  } catch (error) {
    return { error: `Mermaid n'a pas pu être chargé : ${shortMessage(error)}` }
  }
  const prepared = neutralizeMermaid(source)
  if (prepared.trim() === '') return { error: 'Diagramme vide' }
  try {
    applyTheme(mermaid, options.theme, options.colors, options.fontFamily)
    const { svg } = await mermaid.render(options.id, prepared)
    const clean = sanitizeSvg(svg)
    return clean === '' ? { error: 'Rendu vide' } : { svg: clean }
  } catch (error) {
    return { error: shortMessage(error) }
  } finally {
    removeLeftovers(options.id)
  }
}

/**
 * The SVG of a Mermaid source, sanitized, or the short reason it failed.
 * Renders are queued; a cached SVG (same id, source, theme, font and colours)
 * is returned without rendering again.
 */
export function renderMermaid(source: string, opts: MermaidRenderOptions): Promise<MermaidResult> {
  const sizeError = mermaidSourceError(source)
  if (sizeError) return Promise.resolve({ error: sizeError })
  if (!ID_PATTERN.test(opts.id)) {
    return Promise.resolve({ error: 'Identifiant de diagramme invalide' })
  }
  const settings = {
    id: opts.id,
    theme: opts.theme,
    fontFamily: safeFont(opts.fontFamily),
    colors: safeColors(opts.colors),
  }
  const key = JSON.stringify([settings.id, source, settings.theme, settings.fontFamily, settings.colors])
  const cached = opts.cache?.get(key)
  if (cached !== undefined) return Promise.resolve({ svg: cached })
  return serialized(async () => {
    const result = await renderOnce(source, settings)
    if ('svg' in result) opts.cache?.set(key, result.svg)
    return result
  })
}

// ─── Export ──────────────────────────────────────────────────────────────────

const XML_PROLOG = '<?xml version="1.0" encoding="UTF-8"?>\n'

/** A dimension attribute with an absolute value: kept; a relative one (`100%`) or none: `value`. */
function withDimension(attributes: string, name: string, value: number): string {
  const existing = new RegExp(`\\s${name}="([^"]*)"`).exec(attributes)
  if (existing) {
    if (existing[1] !== '' && !existing[1].endsWith('%')) return attributes
    return attributes.replace(existing[0], ` ${name}="${value}"`)
  }
  return ` ${name}="${value}"${attributes}`
}

/**
 * A standalone SVG file from a sanitized diagram: the XML prolog, the SVG
 * namespace (and xlink when used), and explicit width and height from the viewBox.
 */
export function exportableSvg(svg: string): string {
  const body = svg.replace(/^\s*<\?xml[^>]*>\s*/, '')
  const open = /<svg\b([^>]*)>/i.exec(body)
  if (!open) return svg
  let attributes = open[1]
  if (!/\sxmlns="/.test(attributes)) {
    attributes = ` xmlns="http://www.w3.org/2000/svg"${attributes}`
  }
  if (/\sxlink:/.test(attributes) && !/\sxmlns:xlink=/.test(attributes)) {
    attributes = ` xmlns:xlink="http://www.w3.org/1999/xlink"${attributes}`
  }
  const viewBox = /\sviewBox="([^"]*)"/.exec(attributes)?.[1]?.trim().split(/[\s,]+/).map(Number)
  if (viewBox?.length === 4 && viewBox.every(Number.isFinite)) {
    attributes = withDimension(attributes, 'width', viewBox[2])
    attributes = withDimension(attributes, 'height', viewBox[3])
  }
  const start = open.index
  const end = start + open[0].length
  return `${XML_PROLOG}${body.slice(0, start)}<svg${attributes}>${body.slice(end)}`
}
