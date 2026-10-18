// Regression check (Sprint 14.5): the default reading of parsePatch (the Git views) equals the
// frozen c1fe3de parser on real Git output and on generated text. Node environment, no DOM.
import { describe, expect, it } from 'vitest'
import { execFileSync } from 'node:child_process'
import { countPatch as countNow, parsePatch as parseNow } from '../../src/lib/features/changes/patch'
import { countPatch as countOld, parsePatch as parseOld } from './baseline/patch-baseline.ts'

const APP = new URL('../../', import.meta.url).pathname

function gitSamples(): string[] {
  const cwd = APP
  const diff = execFileSync('git', ['diff', 'c1fe3de', '--', '.'], { cwd, encoding: 'utf8', maxBuffer: 256 << 20 })
  const perFile = diff.split(/(?=^diff --git )/m).filter(Boolean)
  const log = execFileSync('git', ['log', '-p', '-n', '30', '--', '.'], { cwd, encoding: 'utf8', maxBuffer: 256 << 20 })
  return [diff, ...perFile, ...log.split(/(?=^commit )/m).filter(Boolean)]
}

function generated(seed: number, count: number): string[] {
  let state = seed >>> 0
  const next = () => {
    state = (Math.imul(state, 1664525) + 1013904223) >>> 0
    return state / 2 ** 32
  }
  const alphabet = ['+', '-', ' ', '@@ -1,3 +1,4 @@', 'diff --git a/x b/x', '\\ No newline at end of file', '', '--- a/x', '+++ b/x', 'texte libre', '-- x', '++ y']
  const out: string[] = []
  for (let i = 0; i < count; i++) {
    const lines: string[] = []
    const n = 1 + Math.floor(next() * 30)
    for (let j = 0; j < n; j++) {
      const pick = alphabet[Math.floor(next() * alphabet.length)] ?? ''
      lines.push(pick.length === 1 ? `${pick}${Math.floor(next() * 99)}` : pick)
    }
    out.push(lines.join('\n') + (next() < 0.5 ? '\n' : ''))
  }
  return out
}

describe('parsePatch default reading equals the c1fe3de parser', () => {
  const samples = [...gitSamples(), ...generated(7, 3000), '', '\n', 'no hunk at all', '@@ -1 +1 @@\n-a\n+b\n']
  it(`${samples.length} samples`, { timeout: 120_000 }, () => {
    const mismatches: number[] = []
    samples.forEach((sample, index) => {
      if (JSON.stringify(parseNow(sample)) !== JSON.stringify(parseOld(sample))) mismatches.push(index)
      if (JSON.stringify(countNow(sample)) !== JSON.stringify(countOld(sample))) mismatches.push(-index - 1)
    })
    expect(mismatches.slice(0, 10)).toEqual([])
  })
})
