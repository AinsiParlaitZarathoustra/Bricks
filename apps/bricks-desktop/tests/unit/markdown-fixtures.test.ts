// Every fixture of the benchmark, under the five streaming splits: no update falls back (a
// fallback would hide a bug of the incremental path) and the final update equals the one-shot
// rendering. Node environment: the fixtures are read with `fs`; the sanitizer is stubbed as in
// the other markdown suites (happy-dom drops the first element of a fragment).
import { describe, expect, it, vi } from 'vitest'
import { MarkdownDocument, renderReference } from '../../src/lib/features/conversation/markdown/document'
import { FIXTURE_NAMES, deltaSplits, loadFixture, type SplitMode } from '../fixtures/markdown/index'

vi.mock('../../src/lib/features/conversation/markdown/sanitize', () => ({
  sanitizeMarkdown: (html: string) => html,
}))

const SPLITS: SplitMode[] = ['char1', 'n7', 'n64', 'boundaries', 'random(3)']
const OPTIONS = { idPrefix: 'fx' }

describe('fixtures under five streaming splits', () => {
  for (const name of FIXTURE_NAMES) {
    it(`${name}: no update falls back, the final update equals the one-shot rendering`, { timeout: 600_000 }, () => {
      const text = loadFixture(name).text
      const reference = JSON.stringify(renderReference(text, OPTIONS))
      for (const split of SPLITS) {
        const doc = new MarkdownDocument(OPTIONS)
        let prefix = ''
        for (const delta of deltaSplits(text, split)) {
          prefix += delta
          doc.update(prefix, false)
          expect(doc.stats.fallback, `${name} ${split} at ${prefix.length}`).toBe(false)
        }
        const final = doc.update(text, true)
        expect(doc.stats.fallback, `${name} ${split} final`).toBe(false)
        expect(JSON.stringify(final), `${name} ${split} final`).toBe(reference)
      }
    })
  }
})
