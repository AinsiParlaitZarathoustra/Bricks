/// <reference types="vite/client" />
// KaTeX for assistant messages. Loaded once, on demand, from the bundle (the
// stylesheet and its fonts are local files Vite emits). Each expression is
// rendered with the bounds below, then sanitized by `sanitizeKatex`.
//
// Bounds (KaTeX options, fixed here, never by the message):
//   - MAX_TEX_LENGTH characters per expression, checked before parsing;
//   - maxExpand 1000: macro expansions per expression (stops `\def\a{\a\a}`);
//   - maxSize 50: largest rule or box, in em;
//   - trust false: \href, \url, \includegraphics and \htmlId are refused;
//   - globalGroup false: a \def stays in its expression; a \gdef writes only
//     the macro table given for the message, never another message's table.
// Errors are returned, never thrown (throwOnError), and strict warnings are
// silenced: a formula that KaTeX only finds unusual still renders.

import { sanitizeKatex } from './sanitize'
import type { MathRenderer } from './types'

/** Longest expression accepted, in characters. */
export const MAX_TEX_LENGTH = 8000

/** Longest error message returned, in characters. */
const MAX_ERROR_LENGTH = 200

type KatexModule = typeof import('katex').default

let loading: Promise<MathRenderer> | null = null

/** The math renderer. Loads KaTeX and its stylesheet the first time; later calls share the result. */
export function loadKatex(): Promise<MathRenderer> {
  loading ??= (async () => {
    const [{ default: katex }] = await Promise.all([
      import('katex'),
      import('katex/dist/katex.min.css'),
    ])
    return renderer(katex)
  })().catch((error: unknown) => {
    // A failed load is retried by the next call.
    loading = null
    throw error
  })
  return loading
}

function renderer(katex: KatexModule): MathRenderer {
  return (tex, display, macros) => {
    if (tex.length > MAX_TEX_LENGTH) {
      return { error: `Formule trop longue (${MAX_TEX_LENGTH} caractères au maximum)` }
    }
    try {
      const html = katex.renderToString(tex, {
        displayMode: display,
        output: 'htmlAndMathml',
        throwOnError: true,
        trust: false,
        strict: 'ignore',
        maxExpand: 1000,
        maxSize: 50,
        macros,
        globalGroup: false,
      })
      const clean = sanitizeKatex(html)
      return clean === '' ? { error: 'Formule non affichable' } : { html: clean }
    } catch (error) {
      return { error: shortMessage(error) }
    }
  }
}

function shortMessage(error: unknown): string {
  const text = error instanceof Error ? error.message : String(error)
  const first = text.split('\n')[0]?.trim() ?? ''
  return (first || 'Formule invalide').slice(0, MAX_ERROR_LENGTH)
}
