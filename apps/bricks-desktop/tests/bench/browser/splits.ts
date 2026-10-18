// Copy of deltaSplits (tests/fixtures/markdown/index.ts) for the browser bundle, which cannot read files.
// Same algorithm, same modes; kept in step by hand (checked by hand, see the regression report).
export type SplitMode = 'char1' | 'n7' | 'n64' | 'boundaries' | `random(${number})`

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

function boundaryCuts(text: string): number[] {
  const cuts = new Set<number>()
  const add = (index: number) => {
    if (index > 0 && index < text.length) cuts.add(index)
  }
  for (const m of text.matchAll(/\$\$/g)) add((m.index ?? 0) + 1)
  for (const m of text.matchAll(/```/g)) add((m.index ?? 0) + 1)
  let lineStart = 0
  for (const line of text.split('\n')) {
    if (line.startsWith('|')) {
      for (let k = 0; k < line.length; k += 1) {
        if (line[k] === '|') add(lineStart + k + 1)
      }
    }
    lineStart += line.length + 1
  }
  for (const m of text.matchAll(/[\u{10000}-\u{10FFFF}]/gu)) add((m.index ?? 0) + 1)
  for (const m of text.matchAll(/\p{L}́/gu)) add((m.index ?? 0) + 1)
  for (let index = 24; index < text.length; index += 24) add(index)
  return [...cuts].sort((a, b) => a - b)
}

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
