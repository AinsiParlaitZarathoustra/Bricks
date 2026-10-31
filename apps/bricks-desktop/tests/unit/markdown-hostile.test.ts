// @vitest-environment happy-dom
// Hostile content through the whole assistant pipeline, with the real sanitizer and the
// real KaTeX renderer. Simulated DOM (happy-dom): the shims below make DOMPurify behave as in
// a browser for this file only; the rendering in Chromium and in the WebView is checked elsewhere.
import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { loadKatex } from '../../src/lib/features/conversation/markdown/math'
import { MarkdownDocument, renderReference } from '../../src/lib/features/conversation/markdown/document'
import { neutralizeMermaid } from '../../src/lib/features/conversation/markdown/mermaid'
import type { RenderedBlock } from '../../src/lib/features/conversation/markdown/types'

// happy-dom answers '' to `Node.prototype.nodeName` on some nodes, and its node iterator stops
// at the first node DOMPurify removes. Both shims give the standard browser answers.
const nodeNameFallback = Object.getOwnPropertyDescriptor(Node.prototype, 'nodeName')
Object.defineProperty(Node.prototype, 'nodeName', {
	configurable: true,
	get(this: Node) {
		switch (this.nodeType) {
			case Node.ELEMENT_NODE:
				return (this as Element).tagName
			case Node.TEXT_NODE:
				return '#text'
			case Node.COMMENT_NODE:
				return '#comment'
			case Node.DOCUMENT_NODE:
				return '#document'
			case Node.DOCUMENT_FRAGMENT_NODE:
				return '#document-fragment'
			default:
				return nodeNameFallback?.get?.call(this) ?? ''
		}
	},
})
const isUnder = (root: Node, node: Node | null): boolean => {
	for (let current = node; current; current = current.parentNode) if (current === root) return true
	return false
}
document.createNodeIterator = ((root: Node, whatToShow: number) => {
	const nodes: Node[] = []
	const collect = (node: Node) => {
		if (whatToShow & (1 << (node.nodeType - 1))) nodes.push(node)
		for (let child = node.firstChild; child; child = child.nextSibling) collect(child)
	}
	collect(root)
	let index = -1
	return {
		root,
		nextNode() {
			while (++index < nodes.length) {
				if (isUnder(root, nodes[index])) return nodes[index]
			}
			return null
		},
		detach() {},
	}
}) as unknown as Document['createNodeIterator']

// The npm scripts run from apps/bricks-desktop (happy-dom gives no file URL to resolve from).
const HOSTILE = readFileSync(join(process.cwd(), 'tests/fixtures/markdown/hostile.md'), 'utf8')
/** Elements that must never reach the DOM of a message (KaTeX's inline SVG and MathML are checked apart). */
const FORBIDDEN_TAGS = ['script', 'iframe', 'object', 'embed', 'img', 'style', 'form', 'input', 'link', 'meta', 'base', 'foreignobject']

function htmlOf(blocks: RenderedBlock[]): string {
	return blocks.map((block) => (block.kind === 'html' ? block.html : '')).join('')
}

function dom(html: string): HTMLElement {
	const root = document.createElement('div')
	root.innerHTML = html
	return root
}

function checkSafe(html: string, allowed: string[] = []): void {
	const root = dom(html)
	for (const tag of FORBIDDEN_TAGS) {
		if (allowed.includes(tag)) continue
		expect(root.querySelectorAll(tag), `<${tag}>`).toHaveLength(0)
	}
	for (const element of root.querySelectorAll('*')) {
		for (const name of element.getAttributeNames()) {
			expect(name.startsWith('on'), `${element.tagName.toLowerCase()} ${name}`).toBe(false)
		}
	}
	for (const element of root.querySelectorAll('[href]')) {
		expect(element.getAttribute('href'), element.tagName).toMatch(/^(?:https?:|mailto:|#)/i)
	}
}

describe('the hostile fixture, through the whole document', () => {
	it('no active element, no handler, no link outside http(s), mailto and in-page', () => {
		const html = htmlOf(renderReference(HOSTILE, { idPrefix: 'h' }))
		checkSafe(html)
		// Not vacuous: the safe content of the fixture is rendered.
		expect(html).toContain('href="mailto:support@example.org"')
		expect(html).toContain('href="http://example.org/page?x=1"')
		expect(html).toContain('Fin du contenu hostile.')
	})

	it('the diagram is handed over as a Mermaid block, and neutralizing it removes its directive and its click', () => {
		const blocks = renderReference(HOSTILE, { idPrefix: 'h' })
		const diagram = blocks.find((block) => block.kind === 'mermaid')
		expect(diagram).toMatchObject({ kind: 'mermaid', complete: true })
		const prepared = neutralizeMermaid(diagram && diagram.kind === 'mermaid' ? diagram.source : '')
		expect(prepared).not.toMatch(/%%\{|securityLevel|click |url\(/)
		expect(prepared).toContain('flowchart LR')
	})

	it('with KaTeX installed, the formulas make no link and no reference', async () => {
		const katex = await loadKatex()
		const doc = new MarkdownDocument({ idPrefix: 'k' })
		doc.setMathRenderer(katex)
		const html = htmlOf(doc.update(HOSTILE, true))
		const root = dom(html)
		expect(root.querySelectorAll('a[href^="javascript"], [href*="javascript"]')).toHaveLength(0)
		expect(root.querySelectorAll('img, iframe, script, object, embed, form')).toHaveLength(0)
		// Not vacuous: the formulas are rendered (MathML is kept; the wrapper span is lost to happy-dom only).
		expect(root.querySelectorAll('math').length).toBeGreaterThan(0)
		checkSafe(html)
	})
})

/** The visible text of rendered HTML. */
function textOf(html: string): string {
	return html.replace(/<[^>]+>/g, '').replace(/&lt;/g, '<').replace(/&gt;/g, '>')
}

describe('hostile snippets, one by one', () => {
	it('raw HTML is shown as text, never as elements', () => {
		// Inline on a paragraph: a block of raw HTML alone would start with a text node (see the happy-dom note above).
		const html = htmlOf(renderReference('Texte <iframe src="https://x.invalid"></iframe> fin', { idPrefix: 'h' }))
		expect(html).toContain('&lt;iframe')
		checkSafe(html)
		expect(textOf(html)).toContain('fin')
	})

	it('a link with a script or data target is shown as its label', () => {
		const html = htmlOf(renderReference('[piège](javascript:alert(1)) et [data](data:text/html,x)', { idPrefix: 'h' }))
		expect(html).toContain('piège')
		expect(html).toContain('data')
		checkSafe(html)
		expect(dom(html).querySelectorAll('a')).toHaveLength(0)
	})

	it('an image is a text placeholder: nothing is loaded', () => {
		const html = htmlOf(renderReference('![logo](https://tracker.invalid/logo.png)', { idPrefix: 'h' }))
		expect(html).toContain('[image : logo]')
		expect(dom(html).querySelectorAll('img')).toHaveLength(0)
	})
})

describe('KaTeX refuses what would make a link or a reference', () => {
	it.each([
		'\\href{https://x.invalid}{\\text{a}}',
		'\\url{https://x.invalid}',
		'\\includegraphics{https://x.invalid/p.png}',
		'\\htmlId{evil}{x}',
	])('%s renders without link, image or id', async (tex) => {
		const katex = await loadKatex()
		const result = katex(tex, false, {})
		expect(result).toHaveProperty('html')
		const root = dom((result as { html: string }).html)
		expect(root.querySelector('a, img, [href], [src]')).toBeNull()
		expect(root.querySelector('[id="evil"]')).toBeNull()
	})

	it('a recursive macro is stopped by the expansion bound, with a short error', async () => {
		const katex = await loadKatex()
		const result = katex('\\def\\a{\\a\\a}\\a', false, {})
		expect(result).toHaveProperty('error')
		expect((result as { error: string }).error.length).toBeLessThanOrEqual(200)
	})

	it('a \\gdef in one message does not reach another message', async () => {
		const katex = await loadKatex()
		const first = new MarkdownDocument({ idPrefix: 'a' })
		const second = new MarkdownDocument({ idPrefix: 'b' })
		first.setMathRenderer(katex)
		second.setMathRenderer(katex)
		expect(htmlOf(first.update('$\\gdef\\qq{ZZ}$\n\n$\\qq$', true))).not.toContain('math-error')
		expect(htmlOf(second.update('$\\qq$', true))).toContain('class="math-error"')
	})

	it('replacing a message removes its old global macros', async () => {
		const katex = await loadKatex()
		const doc = new MarkdownDocument({ idPrefix: 'k' })
		doc.setMathRenderer(katex)
		doc.update('$\\gdef\\qq{ZZ}$\n\n$\\qq$', true)
		expect(htmlOf(doc.update('$\\qq$', true))).toContain('class="math-error"')
	})

	it('a definition later in the message never changes an earlier formula on the next tick', async () => {
		const katex = await loadKatex()
		const doc = new MarkdownDocument({ idPrefix: 'k' })
		doc.setMathRenderer(katex)
		const source = '$\\qq$\n\n$\\gdef\\qq{ZZ}$\n\nSuite.\n\nFin.'
		doc.update(source, false)
		const next = `${source} Encore.`
		expect(doc.update(next, true)).toEqual(renderReference(next, { idPrefix: 'k', math: katex }))
		expect(htmlOf(doc.update(next, true))).toContain('class="math-error"')
	})

	it('changing a macro definition invalidates later formulas at the same offsets', async () => {
		const katex = await loadKatex()
		const doc = new MarkdownDocument({ idPrefix: 'k' })
		doc.setMathRenderer(katex)
		doc.update('$\\gdef\\qq{AA}$\n\n$\\qq$', true)
		const source = '$\\gdef\\qq{BB}$\n\n$\\qq$'
		expect(doc.update(source, true)).toEqual(renderReference(source, { idPrefix: 'k', math: katex }))
	})
})

describe('Mermaid source, neutralized before Mermaid sees it', () => {
	// Mermaid accepts `;` between statements: a statement after one starts no line.
	it('(défense en profondeur) a callback after a ";" statement separator is removed, a web link is kept', () => {
		expect(neutralizeMermaid('flowchart TD\nA-->B; click A call alert()')).not.toMatch(/click/)
		expect(neutralizeMermaid('flowchart TD\nA-->B;click A href "https://x"')).toBe('flowchart TD\nA-->B;click A href "https://x"')
		expect(neutralizeMermaid('flowchart TD\nA-->B;click A href "https://x"; click B call alert()')).not.toMatch(/call/)
	})

	it('keeps no link that runs or loads something', () => {
		for (const url of ['javascript:alert(1)', 'data:text/html,x', 'file:///etc/passwd', 'vbscript:x', '//evil.test']) {
			expect(neutralizeMermaid(`flowchart TD\nA-->B\nclick A href "${url}"`)).not.toMatch(/click/)
		}
	})
})
