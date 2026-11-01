// A ```vega-lite fence: its JSON specification checked, compiled and drawn as an
// SVG string, never mounted live. Vega and Vega-Lite are loaded on demand, at the
// first chart. A JSON specification is not harmless by itself, so:
// - data must be inline (`values`): any `url` is refused before compiling, and
//   the loader refuses every load or link anyway;
// - inline text data (csv, tsv, dsv) is refused: its parser builds functions;
// - Vega expressions run through its AST interpreter, never `eval` or `Function`;
// - the chart is computed in a Web Worker, stopped when it takes too long;
// - the result is an SVG string, sanitized like any other (`sanitizeSvg`), shown
//   in the sandboxed viewer; no tooltip or event handler is installed.
// Pure parts are unit-tested (tests/unit/vega-chart.test.ts).

import { sanitizeSvg } from './sanitize'

/** A specification larger than this is refused (characters). */
export const MAX_VEGA_SOURCE = 200_000
/** A chart that takes longer is given up. */
export const VEGA_TIMEOUT_MS = 10_000

export type VegaResult = { svg: string } | { error: string }

type Json = null | boolean | number | string | Json[] | { [key: string]: Json }
type Spec = { [key: string]: Json }

/** The first key path holding an external resource (`url`), or `null`; inline `values` are data. */
export function externalResource(value: Json, path = ''): string | null {
  if (Array.isArray(value)) {
    for (let i = 0; i < value.length; i++) {
      const found = externalResource(value[i], `${path}[${i}]`)
      if (found) return found
    }
    return null
  }
  if (value && typeof value === 'object') {
    for (const [key, inner] of Object.entries(value)) {
      const here = path ? `${path}.${key}` : key
      if (key === 'url') return here
      // Inline rows are data, whatever their columns are named.
      if (key === 'values' || (key === 'datasets' && !path)) continue
      const found = externalResource(inner, here)
      if (found) return found
    }
  }
  return null
}

/** The first inline text data (csv, tsv, dsv), whose parser builds functions, or `null`. */
export function textData(value: Json, path = ''): string | null {
  if (Array.isArray(value)) {
    for (let i = 0; i < value.length; i++) {
      const found = textData(value[i], `${path}[${i}]`)
      if (found) return found
    }
    return null
  }
  if (value && typeof value === 'object') {
    for (const [key, inner] of Object.entries(value)) {
      const here = path ? `${path}.${key}` : key
      if (key === 'format' && inner && typeof inner === 'object' && !Array.isArray(inner)) {
        const type = (inner as { type?: Json }).type
        if (type === 'csv' || type === 'tsv' || type === 'dsv') return here
      }
      if (key === 'values' && typeof inner === 'string') return here
      if (key === 'values' || (key === 'datasets' && !path)) continue
      const found = textData(inner, here)
      if (found) return found
    }
  }
  return null
}

/** The specification of a fence, or why it cannot be drawn. */
export function parseVegaLite(source: string): { spec: Spec } | { error: string } {
  if (source.length > MAX_VEGA_SOURCE) {
    return { error: `Spécification trop longue (${MAX_VEGA_SOURCE} caractères au maximum)` }
  }
  let value: Json
  try {
    value = JSON.parse(source) as Json
  } catch (e) {
    return { error: `JSON invalide : ${e instanceof Error ? e.message : String(e)}` }
  }
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    return { error: 'La spécification doit être un objet JSON' }
  }
  const external = externalResource(value)
  if (external) {
    return { error: `Ressource externe refusée (${external}) : intègre les données dans « values »` }
  }
  const text = textData(value)
  if (text) {
    return { error: `Données texte refusées (${text}) : donne les lignes en objets JSON dans « values »` }
  }
  return { spec: value }
}

/** The app's colours a chart starts from (hex, as `mermaidColors()` gives them). */
export interface ChartColors {
  foreground?: string
  muted?: string
  border?: string
  primary?: string
  chart1?: string
  chart2?: string
  chart3?: string
  chart4?: string
  chart5?: string
}

function isPlainObject(v: unknown): v is Record<string, unknown> {
  return !!v && typeof v === 'object' && !Array.isArray(v)
}

/** `base` under `over`: what the chart asks for always wins. */
export function mergeConfig(base: Record<string, unknown>, over: unknown): Record<string, unknown> {
  if (!isPlainObject(over)) return base
  const out: Record<string, unknown> = { ...base }
  for (const [key, value] of Object.entries(over)) {
    out[key] = isPlainObject(value) && isPlainObject(out[key]) ? mergeConfig(out[key] as Record<string, unknown>, value) : value
  }
  return out
}

/** The theme of the app as a Vega-Lite `config`: text, axes, grid and series colours. */
export function themeConfig(colors: ChartColors, fontFamily: string): Record<string, unknown> {
  const text = colors.foreground
  const muted = colors.muted ?? text
  const line = colors.border
  const series = [colors.chart2, colors.chart1, colors.chart3, colors.chart4, colors.chart5, colors.primary].filter(
    (c): c is string => typeof c === 'string',
  )
  const config: Record<string, unknown> = {
    background: null,
    font: fontFamily,
    view: { stroke: line ?? null },
    axis: { labelColor: muted, titleColor: text, gridColor: line, domainColor: line, tickColor: line },
    legend: { labelColor: muted, titleColor: text },
    title: { color: text, subtitleColor: muted },
    header: { labelColor: muted, titleColor: text },
    text: { color: text },
  }
  if (series.length > 0) {
    config.range = { category: series }
    config.mark = { color: series[0] }
  }
  return config
}

/**
 * The specification as drawn here: the app's theme under its own config, and
 * the available `width` when it asks for none or for the container's.
 */
export function prepareSpec(spec: Spec, width: number, colors: ChartColors, fontFamily: string): Spec {
  const out: Spec = { ...spec }
  out.config = mergeConfig(themeConfig(colors, fontFamily), spec.config) as Json
  const composite = ['facet', 'hconcat', 'vconcat', 'concat', 'repeat'].some((k) => k in spec)
  if (!composite && (spec.width === undefined || spec.width === 'container') && width > 0) {
    out.width = Math.max(120, Math.floor(width))
    if (spec.autosize === undefined) out.autosize = { type: 'fit', contains: 'padding' }
  }
  // No container height here: the chart keeps Vega-Lite's own.
  if (spec.height === 'container') delete out.height
  return out
}

type Drawn = { svg: string } | { error: string; spec?: boolean }

let worker: Worker | null = null
let nextId = 0
const waiting = new Map<number, (drawn: Drawn) => void>()

/** The worker that draws, created at the first chart and again after a stop. */
function drawer(): Worker {
  if (worker) return worker
  worker = new Worker(new URL('./vega.worker.ts', import.meta.url), { type: 'module' })
  worker.onmessage = (event: MessageEvent<{ id: number } & Drawn>) => {
    const { id, ...drawn } = event.data
    waiting.get(id)?.(drawn)
    waiting.delete(id)
  }
  worker.onerror = (event) => {
    event.preventDefault()
    stopDrawer(`Vega n'a pas pu être chargé : ${event.message || 'erreur du worker'}`)
  }
  return worker
}

/** Stops the worker (and so a runaway chart); what waited for it gets `reason`. */
function stopDrawer(reason: string) {
  worker?.terminate()
  worker = null
  for (const answer of waiting.values()) answer({ error: reason })
  waiting.clear()
}

/** The prepared specification drawn off the page, or why not; stopped after `ms`. */
function drawInWorker(spec: Spec, ms: number): Promise<Drawn> {
  const id = ++nextId
  return new Promise((resolve) => {
    const timer = setTimeout(() => stopDrawer(`rendu trop long (plus de ${ms / 1000} s)`), ms)
    waiting.set(id, (drawn) => {
      clearTimeout(timer)
      resolve(drawn)
    })
    drawer().postMessage({ id, spec })
  })
}

/** In the tests, where no worker exists: the same drawing, in the page. */
async function drawInPage(spec: Spec, ms: number): Promise<Drawn> {
  const { SpecError, drawVegaLite } = await import('./vega-draw')
  try {
    return { svg: await withTimeout(drawVegaLite(spec), ms) }
  } catch (e) {
    return { error: message(e), spec: e instanceof SpecError }
  }
}

function withTimeout<T>(work: Promise<T>, ms: number): Promise<T> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`rendu trop long (plus de ${ms / 1000} s)`)), ms)
    work.then(
      (v) => {
        clearTimeout(timer)
        resolve(v)
      },
      (e) => {
        clearTimeout(timer)
        reject(e)
      },
    )
  })
}

function message(e: unknown): string {
  const text = e instanceof Error ? e.message : String(e)
  return (text.split('\n')[0] ?? '').trim().slice(0, 300) || 'erreur inconnue'
}

/**
 * The chart of `source`, drawn at `width` with the app's `colors`, as a
 * sanitized SVG; or the short reason it was not drawn. Never throws.
 */
export async function renderVegaLite(
  source: string,
  options: { width: number; colors: ChartColors; fontFamily: string },
): Promise<VegaResult> {
  try {
    const parsed = parseVegaLite(source)
    if ('error' in parsed) return parsed
    const spec = prepareSpec(parsed.spec, options.width, options.colors, options.fontFamily)
    let drawn: Drawn
    if (typeof Worker !== 'undefined') drawn = await drawInWorker(spec, VEGA_TIMEOUT_MS)
    // Only the tests draw in the page: the app's bundle carries Vega once, in the worker.
    else if (import.meta.env.MODE === 'test') drawn = await drawInPage(spec, VEGA_TIMEOUT_MS)
    else return { error: 'Graphiques indisponibles ici' }
    if ('error' in drawn) {
      return { error: drawn.spec ? `Spécification invalide : ${message(drawn.error)}` : message(drawn.error) }
    }
    const clean = sanitizeSvg(drawn.svg)
    return clean ? { svg: clean } : { error: 'Rendu vide' }
  } catch (e) {
    return { error: message(e) }
  }
}
