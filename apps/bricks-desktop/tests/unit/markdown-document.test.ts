// @vitest-environment happy-dom
// MarkdownDocument: the streamed blocks equal the one-shot rendering (renderReference),
// frozen blocks are never rendered again, dependencies invalidate only what they touch.
import { describe, expect, it, vi } from 'vitest'
import { MarkdownDocument, renderReference } from '../../src/lib/features/conversation/markdown/document'
import type { MathRenderer, RenderedBlock } from '../../src/lib/features/conversation/markdown/types'

// happy-dom drops the first element of an HTML fragment inside DOMPurify (a test-only DOM
// quirk; browsers keep it). Sanitizing is not under test here: the block HTML is compared as
// marked wrote it, on both sides of every equivalence.
vi.mock('../../src/lib/features/conversation/markdown/sanitize', () => ({
  sanitizeMarkdown: (html: string) => html,
}))

const CORPUS = `# Titre principal

Paragraphe avec **gras**, *italique*, \`code\` et [lien][ref] et [doc].

## Sous-titre très long : Élève & café 😀

- item 1 avec $x^2$
- item 2 avec une note[^1]
  - sous-item $$a+b$$

1. un
2. deux

> [!NOTE]
> Une alerte ici.

> citation
> suite

| a | b |
|---|:-:|
| 1 | $y$ |

\`\`\`ts
const x: number = 1 // $z$ [ref]
\`\`\`

\`\`\`mermaid
graph TD; A-->B
\`\`\`

    indented code

Titre setext
------------

<div>html brut</div>

Voir src/lib/features/conversation/markdown/render.ts:42 et [doc](src/a.ts#L3-L9).

Prix : $5 et $10. Maths \\( \\alpha \\) et \\[ \\int \\].

$$
\\sum_{i=1}^n i
$$

[^1]: Note de bas de page
    suite indentée.

[ref]: https://example.com "Titre"
[doc]: https://doc.example.org

## Sous-titre très long : Élève & café 😀

- [ ] task
- [x] done

Fin [^2] et [^1] encore, [lien](#titre-principal).
`

const OPTIONS = { idPrefix: 'm1' }

function projection(blocks: RenderedBlock[]): unknown[] {
  return blocks.map((block) => (block.kind === 'html' ? [block.kind, block.key, block.html] : [block.kind, block.key, block.source, block.complete]))
}

/**
 * The blocks with every code body reduced to its text. A code block still open while streaming
 * shows its newest text uncoloured: intermediate outputs are compared on text and structure, the
 * final output (and every closed block) strictly.
 */
function uncoloured(blocks: RenderedBlock[]): unknown[] {
  const decode = (text: string) =>
    text.replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&quot;/g, '"').replace(/&#39;|&#x27;/g, "'").replace(/&amp;/g, '&')
  return projection(blocks).map((block) =>
    Array.isArray(block) && block[0] === 'html'
      ? [block[0], block[1], String(block[2]).replace(/<code class="hljs font-mono">([\s\S]*?)<\/code>/g, (_all, inner: string) => `<code class="hljs font-mono">${decode(inner.replace(/<[^>]*>/g, ''))}</code>`)]
      : block,
  )
}

/** Deterministic pseudo-random generator (mulberry32). */
function seeded(seed: number): () => number {
  let state = seed >>> 0
  return () => {
    state = (state + 0x6d2b79f5) >>> 0
    let t = state
    t = Math.imul(t ^ (t >>> 15), t | 1)
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

/** Cut points of a source: every `step` characters, or at random with a fixed seed, or next to a marker. */
function cuts(source: string, mode: 'step1' | 'step7' | 'step64' | 'random' | 'marker'): number[] {
  const points: number[] = []
  if (mode === 'random') {
    const random = seeded(20260518)
    let at = 0
    while (at < source.length) {
      at += 1 + Math.floor(random() * 40)
      points.push(Math.min(at, source.length))
    }
    return points
  }
  if (mode === 'marker') {
    const markers = ['$$', '```', '|', '😀', 'é']
    const found = new Set<number>()
    for (const marker of markers) {
      let index = source.indexOf(marker)
      while (index >= 0) {
        found.add(index + 1)
        found.add(index + marker.length)
        index = source.indexOf(marker, index + 1)
      }
    }
    return [...found].filter((at) => at > 0 && at < source.length).sort((a, b) => a - b)
  }
  const step = mode === 'step1' ? 1 : mode === 'step7' ? 7 : 64
  for (let at = step; at < source.length; at += step) points.push(at)
  return points
}

/** Feeds the source in pieces, as a stream would, and returns the final blocks. */
function streamed(source: string, points: number[]): { doc: MarkdownDocument; final: RenderedBlock[] } {
  const doc = new MarkdownDocument(OPTIONS)
  let last = 0
  for (const at of [...points, source.length]) {
    doc.update(source.slice(0, at), false)
    last = at
  }
  expect(last).toBe(source.length)
  const final = doc.update(source, true)
  return { doc, final }
}

describe('incremental equals the one-shot rendering', () => {
  const reference = renderReference(CORPUS, OPTIONS)

  for (const mode of ['step1', 'step7', 'step64', 'random', 'marker'] as const) {
    it(`final blocks match renderReference when cut ${mode}`, { timeout: 60_000 }, () => {
      const { final } = streamed(CORPUS, cuts(CORPUS, mode))
      expect(projection(final)).toEqual(projection(reference))
    })
  }

  it('a bounded cache changes no output', { timeout: 60_000 }, () => {
    const doc = new MarkdownDocument({ idPrefix: 'm1', cacheLimit: 1 })
    for (let at = 53; at <= CORPUS.length; at += 53) doc.update(CORPUS.slice(0, at), false)
    expect(projection(doc.update(CORPUS, true))).toEqual(projection(reference))
  })

  it('every intermediate update equals the rendering of its prefix', { timeout: 60_000 }, () => {
    const doc = new MarkdownDocument(OPTIONS)
    for (let at = 37; at <= CORPUS.length; at += 37) {
      const prefix = CORPUS.slice(0, at)
      const blocks = doc.update(prefix, false)
      expect(uncoloured(blocks), `prefix of ${at} characters`).toEqual(uncoloured(renderReference(prefix, OPTIONS)))
    }
  })

  it('the corpus renders its features', () => {
    const html = reference.map((block) => (block.kind === 'html' ? block.html : '')).join('')
    expect(html).toContain('<h2 id="m1-titre-principal">')
    expect(html).toContain('<h3 id="m1-sous-titre-tres-long--eleve--cafe-">')
    expect(html).toContain('<h3 id="m1-sous-titre-tres-long--eleve--cafe--1">')
    expect(html).toContain('<h3 id="m1-titre-setext">')
    expect(html).toContain('md-alert-note')
    expect(html).toContain('class="fn-ref"')
    expect(html).toContain('class="md-footnotes"')
    expect(html).toContain('class="file-ref"')
    expect(html).toContain('<table')
    expect(html).toContain('href="https://example.com"')
    expect(reference.some((block) => block.kind === 'mermaid')).toBe(true)
    expect(html).not.toContain('<h1')
  })
})

const FRAGMENTS = [
  'Para text with [a] and [^n] and $m$.',
  '# Heading\n',
  '## Heading [a]\n',
  '- item one\n- item two\n',
  '1. num\n   continued\n',
  '> quote [a]\n> more\n',
  '> [!TIP]\n> tip text\n',
  '```ts\nconst v = [a]\n```\n',
  '~~~mermaid\ngraph TD\n~~~\n',
  '| h1 | h2 |\n|---|---|\n| c1 | c2 |\n',
  '[a]: https://a.example\n',
  '[^n]: note text\n    more\n',
  'Setext\n------\n',
  '<div>\nraw\n</div>\n',
  '    indented code\n',
  '$$\nx = 1\n$$\n',
  'Voir src/a.ts:3 et [f](src/b.ts#L2).\n',
  '---\n',
  'Lazy [a]\nline\n',
  '- [ ] task [a]\n\n  para in item\n',
  '> # quoted heading\n> text\n',
  '[^n] again [^m]\n',
  '[^m]: other\n',
  '[a]: https://a.example "Titre"\n',
  '[a]: https://autre.example\n\n',
  '[^n]: note again\n',
]

describe('random compositions of blocks', () => {
  it('every prefix of a random message equals its one-shot rendering', { timeout: 120_000 }, () => {
    const random = seeded(4242)
    for (let document = 0; document < 60; document += 1) {
      let source = ''
      const count = 2 + Math.floor(random() * 6)
      for (let k = 0; k < count; k += 1) {
        source += (FRAGMENTS[Math.floor(random() * FRAGMENTS.length)] ?? '') + (random() < 0.5 ? '\n' : '')
      }
      const doc = new MarkdownDocument(OPTIONS)
      for (let at = 5; at <= source.length + 4; at += 5) {
        const prefix = source.slice(0, Math.min(at, source.length))
        expect(uncoloured(doc.update(prefix, false)), JSON.stringify(prefix)).toEqual(uncoloured(renderReference(prefix, OPTIONS)))
      }
      expect(projection(doc.update(source, true)), JSON.stringify(source)).toEqual(projection(renderReference(source, OPTIONS)))
    }
  })
})

describe('repeated definitions', () => {
  const REPRODUCERS = [
    '[a]: https://x.y\n\n[a]: https://x.y\n',
    '[a]: https://x.y\n[a]: https://z.w\n',
    'p [a]\n\n[a]: https://x.y "T"\n[b]: https://q.r\n\n[a]: https://x.y "T"\n[b]: https://q.r\n',
    '[a]: https://x.y "T"\n[a]: https://x.y "T"',
    '[^1]: un\n\n[^1]: deux\n\nTexte[^1].\n',
  ]

  for (const source of REPRODUCERS) {
    it(`renders ${JSON.stringify(source)} like the one-shot rendering, with no fallback`, () => {
      const doc = new MarkdownDocument(OPTIONS)
      const final = doc.update(source, true)
      expect(doc.stats.fallback).toBe(false)
      expect(projection(final)).toEqual(projection(renderReference(source, OPTIONS)))
      const streamed = new MarkdownDocument(OPTIONS)
      for (let at = 1; at <= source.length; at += 1) streamed.update(source.slice(0, at), false)
      expect(projection(streamed.update(source, true))).toEqual(projection(final))
    })
  }

  it('the first definition wins, a repeated one renders nothing', () => {
    const html = renderReference('Voir [a].\n\n[a]: https://x.y\n\n[a]: https://z.w\n', OPTIONS)
      .map((block) => (block.kind === 'html' ? block.html : ''))
      .join('')
    expect(html).toContain('href="https://x.y"')
    expect(html).not.toContain('z.w')
  })
})

describe('update never throws', () => {
  it('a math renderer that answers nonsense falls back to the escaped source, then recovers', () => {
    const doc = new MarkdownDocument(OPTIONS)
    doc.setMathRenderer((() => 42) as unknown as MathRenderer)
    const source = 'Un $x$ <b>brut</b>\nligne deux'
    const broken = doc.update(source, false)
    expect(doc.stats.fallback).toBe(true)
    expect(broken).toEqual([{ key: 'source', kind: 'html', html: '<p>Un $x$ &lt;b&gt;brut&lt;/b&gt;<br>ligne deux</p>' }])
    const working: MathRenderer = (tex) => ({ html: `<i>${tex}</i>` })
    doc.setMathRenderer(working)
    doc.update(source, false)
    expect(doc.stats.fallback).toBe(false)
    expect(projection(doc.update(source, true))).toEqual(projection(renderReference(source, { ...OPTIONS, math: working })))
  })
})

describe('late definitions', () => {
  it('a link definition after its paragraph resolves the paragraph', () => {
    const source = 'Voir [doc][d].\n\nPlus.\n\n[d]: https://x.y\n'
    const { final } = streamed(source, cuts(source, 'step1'))
    const html = final.map((block) => (block.kind === 'html' ? block.html : '')).join('')
    expect(html).toContain('href="https://x.y"')
    expect(projection(final)).toEqual(projection(renderReference(source, OPTIONS)))
  })

  it('a footnote definition after its reference numbers and links it', () => {
    const source = 'Une note[^a].\n\nAutre[^b] et [^a].\n\n[^b]: Contenu B\n\n[^a]: Contenu A\n'
    const { final } = streamed(source, cuts(source, 'step7'))
    const html = final.map((block) => (block.kind === 'html' ? block.html : '')).join('')
    expect(html).toContain('id="m1-fnref:a-1"')
    expect(html).toContain('id="m1-fnref:a-2"')
    expect(html.indexOf('href="#m1-fn:a"')).toBeLessThan(html.indexOf('href="#m1-fn:b"'))
    expect(projection(final)).toEqual(projection(renderReference(source, OPTIONS)))
  })

  it('an unresolved footnote reference stays literal text and an unused definition is not shown', () => {
    const blocks = renderReference('Texte [^zz] ici.\n\n[^unused]: rien\n', OPTIONS)
    const html = blocks.map((block) => (block.kind === 'html' ? block.html : '')).join('')
    expect(html).toContain('[^zz]')
    expect(html).not.toContain('md-footnotes')
    expect(html).not.toContain('unused')
  })
})

describe('frozen blocks are not rendered again', () => {
  it('a highlighted block before the tail is not highlighted again when text is added', () => {
    const doc = new MarkdownDocument(OPTIONS)
    doc.update('```js\nconst a = 1\n```\n\nPara A\n\nPara B', false)
    doc.update('```js\nconst a = 1\n```\n\nPara A\n\nPara B and more', false)
    const stats = doc.stats
    expect(stats.highlightCalls).toBe(0)
    expect(stats.renderedBlocks).toBe(1)
    expect(stats.reusedBlocks).toBeGreaterThan(0)
  })

  it('an unrelated update re-renders only the blocks of the tail', () => {
    const doc = new MarkdownDocument(OPTIONS)
    doc.update('Un.\n\nDeux.\n\nTrois.\n\nQuatre', false)
    doc.update('Un.\n\nDeux.\n\nTrois.\n\nQuatre plus', false)
    expect(doc.stats.renderedBlocks).toBeLessThanOrEqual(2)
    expect(doc.stats.fullLex).toBe(false)
  })

  it('a new link definition re-renders exactly the frozen block that uses its label', () => {
    const doc = new MarkdownDocument(OPTIONS)
    const first = doc.update('Voir [x] ici.\n\nAutre [y].\n\nFin.\n\nDernier.\n', false)
    const second = doc.update('Voir [x] ici.\n\nAutre [y].\n\nFin.\n\nDernier.\n\n[x]: https://x\n', false)
    // "Voir [x]" is frozen and uses the new label: one invalidation, one render. "Autre [y]" is
    // frozen and untouched: same object. "Dernier." gains a blank line (its raw changes): a tail
    // block re-renders, which is the only other render.
    expect(doc.stats.invalidatedBlocks).toBe(1)
    expect(doc.stats.renderedBlocks).toBe(2)
    expect(second[1]).toBe(first[1])
    expect(second[0]).not.toBe(first[0])
    const html = second.map((block) => (block.kind === 'html' ? block.html : '')).join('')
    expect(html).toContain('href="https://x"')
  })

  it('the final update keeps the objects of unchanged blocks', () => {
    const doc = new MarkdownDocument(OPTIONS)
    const source = '# Titre\n\nUn paragraphe.\n\nUn autre.\n\nFin\n'
    const before = doc.update(source, false)
    const after = doc.update(source, true)
    expect(after).toHaveLength(before.length)
    for (let index = 0; index < before.length; index += 1) expect(after[index]).toBe(before[index])
  })

  it('a replaced source is lexed from scratch', () => {
    const doc = new MarkdownDocument(OPTIONS)
    doc.update('Premier texte.\n\nSuite.', false)
    const blocks = doc.update('Autre chose.', false)
    expect(doc.stats.fullLex).toBe(true)
    expect(projection(blocks)).toEqual(projection(renderReference('Autre chose.', OPTIONS)))
  })
})

describe('an open code block while streaming', () => {
  it('is highlighted again only once its unhighlighted tail is long, and equals the one-shot rendering once closed', () => {
    const code = Array.from({ length: 400 }, (_, k) => `const v${k} = ${k}; // ligne ${k}`).join('\n')
    const open = `Avant.\n\n\`\`\`ts\n${code}`
    const doc = new MarkdownDocument(OPTIONS)
    let calls = 0
    let updates = 0
    for (let at = 6; at <= open.length; at += 60) {
      doc.update(open.slice(0, at), false)
      calls += doc.stats.highlightCalls
      updates += 1
    }
    // About one highlight per 1500 characters of code, not one per update.
    expect(calls).toBeLessThanOrEqual(Math.ceil(open.length / 1500) + 2)
    expect(updates).toBeGreaterThan(calls * 5)
    const closed = `${open}\n\`\`\`\n`
    expect(projection(doc.update(closed, false))).toEqual(projection(renderReference(closed, OPTIONS)))
    expect(doc.stats.highlightCalls).toBeGreaterThan(0)
  })
})

describe('a truncated final stream keeps what it received', () => {
  it('an open fence and an open formula are shown, the mermaid fence is not complete', () => {
    const doc = new MarkdownDocument(OPTIONS)
    const code = doc.update('Avant.\n\n```ts\nconst a = 1', true)
    expect(code.map((block) => (block.kind === 'html' ? block.html : '')).join('')).toContain('const')
    const formula = doc.update('Avant.\n\n$$\nx^2 + y', true)
    expect(formula.map((block) => (block.kind === 'html' ? block.html : '')).join('')).toContain('x^2 + y')
    const mermaid = doc.update('```mermaid\ngraph TD\n', true)
    expect(mermaid).toEqual([{ key: 'b0', kind: 'mermaid', source: 'graph TD', complete: false }])
  })
})

describe('math macros belong to their document', () => {
  it('replays cached macro mutations in source order without rendering frozen math again', () => {
    const math: MathRenderer = (tex, _display, macros) => {
      if (tex === 'define') macros.value = 'defined'
      return { html: `<i>${macros.value ?? 'unset'}</i>` }
    }
    const doc = new MarkdownDocument(OPTIONS)
    doc.setMathRenderer(math)
    const source = '$value$\n\n$define$\n\n$value$\n\nTexte.\n\nFin.'
    const first = doc.update(source, false)
    const next = `${source} Suite.`
    const second = doc.update(next, false)
    expect(doc.stats.mathRenders).toBe(0)
    expect(second.slice(0, 3)).toEqual(first.slice(0, 3))
    expect(second[0]).toBe(first[0])
    expect(doc.update(next, true)).toEqual(renderReference(next, { ...OPTIONS, math }))
    expect(doc.stats.mathRenders).toBe(0)
  })

  it('a formula found only in a referenced footnote requests the math renderer', () => {
    const doc = new MarkdownDocument(OPTIONS)
    const source = 'Texte[^1].\n\n[^1]: Une formule $x^2$.'
    doc.update(source, true)
    expect(doc.needsMath).toBe(true)
    const math: MathRenderer = (tex) => ({ html: `<i>${tex}</i>` })
    doc.setMathRenderer(math)
    const blocks = doc.update(source, true)
    expect(doc.needsMath).toBe(false)
    expect(blocks).toEqual(renderReference(source, { ...OPTIONS, math }))
    expect(JSON.stringify(blocks)).toContain('<i>x^2</i>')
  })

  it('each document passes its own macro table to the renderer', () => {
    const seen: Record<string, string>[] = []
    const renderer: MathRenderer = (_tex, _display, macros) => {
      seen.push(macros)
      return { html: '' }
    }
    const a = new MarkdownDocument({ idPrefix: 'a' })
    const b = new MarkdownDocument({ idPrefix: 'b' })
    a.setMathRenderer(renderer)
    b.setMathRenderer(renderer)
    a.update('$x$', true)
    b.update('$y$', true)
    expect(seen).toHaveLength(2)
    expect(seen[0]).not.toBe(seen[1])
  })
})

describe('headings, ids and the heading list', () => {
  it('duplicate headings get -1, -2 in document order, and the list matches the HTML', () => {
    const source = '## Notes\n\nA.\n\n## Notes\n\nB.\n\n## Notes\n\nC.\n\nFin.\n'
    const doc = new MarkdownDocument(OPTIONS)
    const blocks = doc.update(source, true)
    const html = blocks.map((block) => (block.kind === 'html' ? block.html : '')).join('')
    expect(html).toContain('id="m1-notes"')
    expect(html).toContain('id="m1-notes-1"')
    expect(html).toContain('id="m1-notes-2"')
    expect(doc.headings).toEqual([
      { id: 'm1-notes', text: 'Notes', level: 2 },
      { id: 'm1-notes-1', text: 'Notes', level: 2 },
      { id: 'm1-notes-2', text: 'Notes', level: 2 },
    ])
  })

  it('a frozen heading keeps its id while the tail grows', () => {
    const doc = new MarkdownDocument(OPTIONS)
    doc.update('# Premier\n\nTexte\n\nSuite', false)
    doc.update('# Premier\n\nTexte\n\nSuite et plus', false)
    expect(doc.headings.map((heading) => heading.id)).toEqual(['m1-premier'])
    expect(doc.headings[0]?.level).toBe(1)
  })
})

describe('documents are independent', () => {
  it('two documents keep their own prefixes and math renderers', () => {
    const a = new MarkdownDocument({ idPrefix: 'a' })
    const b = new MarkdownDocument({ idPrefix: 'b' })
    const renderer: MathRenderer = (tex) => ({ html: `<span class="k">${tex}</span>` })
    a.setMathRenderer(renderer)
    const blocksA = a.update('# Titre\n\n$x$', true)
    const blocksB = b.update('# Titre\n\n$x$', true)
    const htmlA = blocksA.map((block) => (block.kind === 'html' ? block.html : '')).join('')
    const htmlB = blocksB.map((block) => (block.kind === 'html' ? block.html : '')).join('')
    expect(htmlA).toContain('id="a-titre"')
    expect(htmlB).toContain('id="b-titre"')
    expect(htmlA).toContain('<span class="k">x</span>')
    expect(htmlB).toContain('class="math-src"')
    expect(a.needsMath).toBe(false)
    expect(b.needsMath).toBe(true)
  })

  it('a math renderer installed later re-renders the blocks with math only', () => {
    const doc = new MarkdownDocument(OPTIONS)
    doc.update('Texte simple.\n\n$x$\n\nFin', false)
    expect(doc.needsMath).toBe(true)
    doc.setMathRenderer((tex) => ({ html: `<i>${tex}</i>` }))
    const html = doc.update('Texte simple.\n\n$x$\n\nFin', false)
      .map((block) => (block.kind === 'html' ? block.html : ''))
      .join('')
    expect(html).toContain('<i>x</i>')
    expect(doc.needsMath).toBe(false)
  })

  it('a math error is shown as text with its message', () => {
    const blocks = renderReference('Un $x$.', { idPrefix: 'e', math: () => ({ error: 'tex <invalide>' }) })
    const html = blocks.map((block) => (block.kind === 'html' ? block.html : '')).join('')
    expect(html).toContain('class="math-error" title="tex &lt;invalide&gt;"')
  })
})
