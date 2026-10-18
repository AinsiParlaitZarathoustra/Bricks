// Markdown fixtures for the Sprint 14.5 rendering benchmark and tests.
// Files in this directory are static, deterministic and fictional (no real data).
// Node loads them with `fs`; deltaSplits() cuts a text into streaming deltas.
// Node runs this file with type stripping (no build step): keep the syntax erasable.

import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

export const FIXTURE_DIR = fileURLToPath(new URL('.', import.meta.url))

/** Fixture names, in the order the benchmark reports them. */
export const FIXTURE_NAMES = [
  'simple',
  'rust-long',
  'nested',
  'tables',
  'late-defs',
  'math',
  'mermaid',
  'alerts',
  'files-diff',
  'hostile',
  'mixed-large',
] as const

export type FixtureName = (typeof FIXTURE_NAMES)[number]

export interface Fixture {
  name: FixtureName
  text: string
  bytes: number
  lines: number
}

export function loadFixture(name: FixtureName): Fixture {
  const text = readFileSync(`${FIXTURE_DIR}${name}.md`, 'utf8')
  return { name, text, bytes: Buffer.byteLength(text, 'utf8'), lines: text.split('\n').length }
}

export function loadAllFixtures(): Fixture[] {
  return FIXTURE_NAMES.map(loadFixture)
}

/** Split modes: one code point, fixed 7 or 64 code points, seeded random sizes 1-12 code points, or cuts at fragile positions. */
export type SplitMode = 'char1' | 'n7' | 'n64' | 'boundaries' | `random(${number})`

/** Mulberry32: small deterministic PRNG, returns floats in [0, 1). */
export function prng(seed: number): () => number {
  let state = seed >>> 0
  return () => {
    state = (state + 0x6d2b79f5) >>> 0
    let t = state
    t = Math.imul(t ^ (t >>> 15), t | 1)
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

function splitByCodePoints(text: string, sizeOf: () => number): string[] {
  const points = Array.from(text)
  const out: string[] = []
  let i = 0
  while (i < points.length) {
    const size = Math.max(1, sizeOf())
    out.push(points.slice(i, i + size).join(''))
    i += size
  }
  return out
}

/** UTF-16 cut positions (strictly inside the text) where a delta boundary is fragile. */
function boundaryCuts(text: string): number[] {
  const cuts = new Set<number>()
  const add = (index: number) => {
    if (index > 0 && index < text.length) cuts.add(index)
  }
  // Middle of a `$$` display-math delimiter.
  for (const m of text.matchAll(/\$\$/g)) add((m.index ?? 0) + 1)
  // Middle of a code fence marker.
  for (const m of text.matchAll(/```/g)) add((m.index ?? 0) + 1)
  // Inside a table row: right after each pipe of a row that starts with `|`.
  let lineStart = 0
  for (const line of text.split('\n')) {
    if (line.startsWith('|')) {
      for (let k = 0; k < line.length; k += 1) {
        if (line[k] === '|') add(lineStart + k + 1)
      }
    }
    lineStart += line.length + 1
  }
  // Between the two code units of a surrogate pair (astral characters such as emoji).
  for (const m of text.matchAll(/[\u{10000}-\u{10FFFF}]/gu)) add((m.index ?? 0) + 1)
  // Between a base letter and a combining accent (decomposed accented character).
  for (const m of text.matchAll(/\p{L}́/gu)) add((m.index ?? 0) + 1)
  // Plus a regular grid so that long unmarked passages still get several deltas.
  for (let index = 24; index < text.length; index += 24) add(index)
  return [...cuts].sort((a, b) => a - b)
}

/**
 * Splits a text into deltas whose concatenation is exactly the text.
 * - `char1`: one code point per delta.
 * - `n7` / `n64`: fixed 7 / 64 code points per delta.
 * - `random(seed)`: seeded sizes between 1 and 12 code points (token-like).
 * - `boundaries`: cuts inside `$$`, code fences, table rows, astral characters
 *   (surrogate pairs), decomposed accents, plus a 24-unit grid.
 */
export function deltaSplits(text: string, mode: SplitMode): string[] {
  if (mode === 'char1') return splitByCodePoints(text, () => 1)
  if (mode === 'n7') return splitByCodePoints(text, () => 7)
  if (mode === 'n64') return splitByCodePoints(text, () => 64)
  const random = /^random\((\d+)\)$/.exec(mode)
  if (random) {
    const next = prng(Number(random[1]))
    return splitByCodePoints(text, () => 1 + Math.floor(next() * 12))
  }
  if (mode === 'boundaries') {
    const out: string[] = []
    let previous = 0
    for (const cut of [...boundaryCuts(text), text.length]) {
      out.push(text.slice(previous, cut))
      previous = cut
    }
    return out
  }
  throw new Error(`Unknown split mode: ${String(mode)}`)
}
