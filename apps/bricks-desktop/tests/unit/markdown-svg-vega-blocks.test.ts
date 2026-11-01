// @vitest-environment happy-dom
// ```svg and ```vega-lite fences of assistant messages: top-level fences become
// dedicated blocks (kind 'svg' / 'vega'), open while streaming, closed at the end;
// anything else (nested, other languages) stays HTML. Streams equal the one-shot
// rendering, and the keys of a block stay stable while it grows. SvgBlock.svelte
// (mounted here, sanitizing mocked as above) draws a closed svg fence or shows why not.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { flushSync, mount, unmount } from 'svelte'
import { MarkdownDocument, renderReference } from '../../src/lib/features/conversation/markdown/document'
import type { RenderedBlock } from '../../src/lib/features/conversation/markdown/types'
import SvgBlock from '../../src/lib/features/conversation/markdown/SvgBlock.svelte'
import { MAX_SVG_LENGTH } from '../../src/lib/features/conversation/markdown/svg-viewer'

vi.mock('../../src/lib/features/conversation/markdown/sanitize', () => ({
  sanitizeMarkdown: (html: string) => html,
  sanitizeSvg: (svg: string) => svg,
}))

const OPTIONS = { idPrefix: 'm1' }

const SVG_DOC = '<svg viewBox="0 0 10 10"><rect width="10" height="10"/></svg>'
const VEGA_DOC = '{"mark":"point","data":{"values":[{"x":1,"y":2}]},"encoding":{"x":{"field":"x","type":"quantitative"}}}'

function kinds(blocks: RenderedBlock[]): string[] {
  return blocks.map((block) => block.kind)
}

function special(blocks: RenderedBlock[]): Array<RenderedBlock & { kind: 'svg' | 'vega' }> {
  return blocks.filter((block): block is RenderedBlock & { kind: 'svg' | 'vega' } => block.kind === 'svg' || block.kind === 'vega')
}

/** Every top-level block as data, the html ones compared by their markup. */
function projection(blocks: RenderedBlock[]): unknown[] {
  return blocks.map((block) =>
    block.kind === 'html' ? [block.kind, block.key, block.html] : [block.kind, block.key, block.source, block.complete],
  )
}

describe('top-level svg and vega-lite fences', () => {
  it('turns a closed ```svg fence into an svg block, complete', () => {
    const blocks = renderReference('Avant\n\n```svg\n' + SVG_DOC + '\n```\n\nAprès\n', OPTIONS)
    expect(kinds(blocks)).toEqual(['html', 'svg', 'html'])
    const drawing = special(blocks)[0]
    expect(drawing).toMatchObject({ kind: 'svg', complete: true })
    expect(drawing?.kind === 'svg' && drawing.source.trim()).toBe(SVG_DOC)
  })

  it.each(['```vega-lite', '```vegalite', '```VEGA-LITE', '```vega-lite json', '```vegalite title=x'])(
    'turns a closed %s fence into a vega block, complete',
    (opening) => {
      const blocks = renderReference(`${opening}\n${VEGA_DOC}\n\`\`\`\n`, OPTIONS)
      expect(kinds(blocks)).toEqual(['vega'])
      expect(blocks[0]).toMatchObject({ kind: 'vega', complete: true })
    },
  )

  it('keeps other languages as HTML: vega, xml, svg+xml, and a mermaid fence stays mermaid', () => {
    expect(kinds(renderReference('```vega\n{}\n```\n', OPTIONS))).toEqual(['html'])
    expect(kinds(renderReference('```xml\n<svg/>\n```\n', OPTIONS))).toEqual(['html'])
    expect(kinds(renderReference('```svg+xml\n<svg/>\n```\n', OPTIONS))).toEqual(['html'])
    expect(kinds(renderReference('```mermaid\ngraph TD; A-->B\n```\n', OPTIONS))).toEqual(['mermaid'])
  })

  it('does not turn a svg or vega fence nested in a list or a quote into a block', () => {
    const inList = renderReference('- élément\n\n  ```svg\n  ' + SVG_DOC + '\n  ```\n', OPTIONS)
    const inQuote = renderReference('> ```vega-lite\n> ' + VEGA_DOC + '\n> ```\n', OPTIONS)
    expect(special(inList)).toEqual([])
    expect(special(inQuote)).toEqual([])
  })

  it('keeps the source of an unclosed svg fence open (complete false)', () => {
    const blocks = new MarkdownDocument(OPTIONS).update('Intro\n\n```svg\n<svg viewBox="0 0 10 10">', false)
    expect(blocks[blocks.length - 1]).toMatchObject({ kind: 'svg', complete: false })
  })

  it('keeps the source of an unclosed vega-lite fence open (complete false)', () => {
    const blocks = new MarkdownDocument(OPTIONS).update('```vega-lite\n{"mark":"po', false)
    expect(blocks).toHaveLength(1)
    expect(blocks[0]).toMatchObject({ kind: 'vega', complete: false })
    expect(blocks[0]?.kind === 'vega' && blocks[0].source).toContain('"mark":"po')
  })

  it('does not render an open vega fence as HTML, even when it holds markup', () => {
    const blocks = new MarkdownDocument(OPTIONS).update('```vega-lite\n{"title":"<img src=x onerror=alert(1)>"', false)
    expect(kinds(blocks)).toEqual(['vega'])
  })

  it('closes a streamed svg fence when its closing line arrives', () => {
    const doc = new MarkdownDocument(OPTIONS)
    const open = doc.update('```svg\n' + SVG_DOC, false)
    expect(open[0]).toMatchObject({ kind: 'svg', complete: false })
    const closed = doc.update('```svg\n' + SVG_DOC + '\n```\n', false)
    expect(closed[0]).toMatchObject({ kind: 'svg', complete: true })
    expect(closed[0]?.key).toBe(open[0]?.key)
  })

  it('keeps the key of a vega block while its source grows', () => {
    const doc = new MarkdownDocument(OPTIONS)
    const keys = ['```vega-lite\n{"mark"', '```vega-lite\n{"mark":"point"', '```vega-lite\n{"mark":"point"}\n```\n'].map(
      (text) => doc.update(text, false)[0]?.key,
    )
    expect(new Set(keys).size).toBe(1)
  })
})

describe('streams of svg and vega-lite fences', () => {
  const SOURCE = [
    'Un graphique :',
    '',
    '```vega-lite',
    VEGA_DOC,
    '```',
    '',
    'Un dessin :',
    '',
    '```svg',
    SVG_DOC,
    '```',
    '',
    'Fin.',
    '',
  ].join('\n')

  it('streams one character at a time to the one-shot rendering', () => {
    const doc = new MarkdownDocument(OPTIONS)
    for (let end = 1; end <= SOURCE.length; end += 1) doc.update(SOURCE.slice(0, end), false)
    expect(projection(doc.update(SOURCE, true))).toEqual(projection(renderReference(SOURCE, OPTIONS)))
  })

  it('streams in seeded random chunks to the one-shot rendering', () => {
    let state = 20260601
    const random = () => {
      state = (Math.imul(state, 1664525) + 1013904223) >>> 0
      return state / 4294967296
    }
    const doc = new MarkdownDocument(OPTIONS)
    let at = 0
    while (at < SOURCE.length) {
      at = Math.min(SOURCE.length, at + 1 + Math.floor(random() * 23))
      doc.update(SOURCE.slice(0, at), false)
    }
    expect(projection(doc.update(SOURCE, true))).toEqual(projection(renderReference(SOURCE, OPTIONS)))
  })

  it('reports incomplete charts while open and complete charts once closed, never the reverse', () => {
    const doc = new MarkdownDocument(OPTIONS)
    const states: string[] = []
    for (let end = 1; end <= SOURCE.length; end += 1) {
      const blocks = doc.update(SOURCE.slice(0, end), false)
      const current = special(blocks).at(-1)
      if (current) states.push(`${current.kind}:${current.complete ? 'done' : 'open'}`)
    }
    expect(states).toContain('vega:open')
    expect(states).toContain('vega:done')
    expect(states).toContain('svg:open')
    expect(states).toContain('svg:done')
    // Once a chart is done, its state never falls back to open.
    const vegaStates = states.filter((s) => s.startsWith('vega:'))
    const firstDone = vegaStates.indexOf('vega:done')
    expect(vegaStates.slice(firstDone).every((s) => s === 'vega:done')).toBe(true)
  })

  it('keeps the earlier closed svg block unchanged when the message grows after it', () => {
    const doc = new MarkdownDocument(OPTIONS)
    const before = doc.update('```svg\n' + SVG_DOC + '\n```\n\nSuite', false)
    const after = doc.update('```svg\n' + SVG_DOC + '\n```\n\nSuite et plus', false)
    expect(special(after)[0]).toEqual(special(before)[0])
  })
})

describe('SvgBlock: a closed svg fence drawn, or its source shown', () => {
	const ALERT = 'Dessin non rendu : la racine doit être un élément <svg>'
	let target: HTMLDivElement
	let app: ReturnType<typeof mount> | null = null

	beforeEach(() => {
		target = document.createElement('div')
		document.body.appendChild(target)
	})

	afterEach(() => {
		if (app) void unmount(app)
		app = null
		target.remove()
	})

	function show(source: string, complete = true): void {
		app = mount(SvgBlock, { target, props: { source, complete } })
		flushSync()
	}

	function alertText(): string | null {
		return target.querySelector('[role="alert"]')?.textContent ?? null
	}

	it('refuses a root that is not an svg element, and shows the source', () => {
		const source = '<div><svg viewBox="0 0 10 10"/></div>'
		show(source)
		expect(alertText()).toBe(ALERT)
		expect(target.querySelector('pre code')?.textContent).toBe(source)
		expect(target.querySelector('iframe')).toBeNull()
	})

	it('refuses text that is not markup', () => {
		show('Bonjour le monde')
		expect(alertText()).toBe(ALERT)
		expect(target.querySelector('pre code')?.textContent).toBe('Bonjour le monde')
	})

	it('refuses a source longer than 5 MiB', () => {
		show('<svg>'.padEnd(MAX_SVG_LENGTH + 1, 'x'))
		expect(alertText()).toBe('Dessin non rendu : SVG trop volumineux (5 Mio au maximum)')
	})

	it('shows the source of an unfinished fence without an error', () => {
		show('<div>', false)
		expect(alertText()).toBeNull()
		expect(target.querySelector('pre code')?.textContent).toBe('<div>')
	})

	it('accepts an xml declaration and a comment before the root', () => {
		show('<?xml version="1.0" encoding="UTF-8"?>\n<!-- schéma -->\n<svg viewBox="0 0 10 10"><rect width="10" height="10"/></svg>')
		expect(alertText()).toBeNull()
		expect(target.querySelector('iframe')).not.toBeNull()
	})

	it('accepts a doctype and a comment before the root', () => {
		show('<!DOCTYPE svg>\n<!-- dessin -->\n<svg viewBox="0 0 10 10"><rect width="10" height="10"/></svg>')
		expect(alertText()).toBeNull()
		expect(target.querySelector('iframe')).not.toBeNull()
	})
})
