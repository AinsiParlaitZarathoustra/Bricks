// @vitest-environment happy-dom
// The icons of code block headers (render.ts, document.ts): the file or the language of a fence
// names its icon, while the block streams and after it closes; the copy button copies the code
// only; a model's raw HTML never makes an image; Mermaid and diff fences are unchanged. The real
// sanitizer runs here, with the same DOM shims as markdown-hostile.test.ts.
import { describe, expect, it, vi } from 'vitest'
import { markdownClicks } from '../../src/lib/features/conversation/markdown/clicks'
import { MarkdownDocument, renderReference } from '../../src/lib/features/conversation/markdown/document'
import {
	codeShell,
	fenceIconId,
	finishHtml,
	withCodeIcons,
	type MathSettings,
} from '../../src/lib/features/conversation/markdown/render'
import { sanitizeMarkdown } from '../../src/lib/features/conversation/markdown/sanitize'
import type { RenderedBlock } from '../../src/lib/features/conversation/markdown/types'
import { isIconId } from '../../src/lib/features/file-icons/resolve'

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

const OPTIONS = { idPrefix: 'code' }
const NO_MATH: MathSettings = { renderer: null, macros: {}, renders: { count: 0 } }
const IMG = /<img class="file-icon"[^>]*>/g

/** The HTML of every html block, in order (mermaid and diff blocks contribute nothing). */
function htmlOf(blocks: RenderedBlock[]): string {
	return blocks.map((block) => (block.kind === 'html' ? block.html : '')).join('')
}

function icons(html: string): string[] {
	return html.match(IMG) ?? []
}

function srcOf(img: string): string {
	return /src="([^"]+)"/.exec(img)?.[1] ?? ''
}

describe('a code fence names its icon while it streams and after it closes', () => {
	it('a rust fence: one icon in the header while the fence is open', () => {
		const doc = new MarkdownDocument(OPTIONS)
		const open = htmlOf(doc.update('```rust\nfn main() {}\n', false))
		expect(icons(open)).toHaveLength(1)
		expect(srcOf(icons(open)[0]!)).toMatch(/file-icons\/svg\/rust\.svg$/)
		doc.dispose()
	})

	it('the same header once the fence has closed, with the same icon', () => {
		const doc = new MarkdownDocument(OPTIONS)
		doc.update('```rust\nfn main() {}\n', false)
		const closed = htmlOf(doc.update('```rust\nfn main() {}\n```\n', true))
		expect(icons(closed)).toHaveLength(1)
		expect(srcOf(icons(closed)[0]!)).toMatch(/file-icons\/svg\/rust\.svg$/)
		doc.dispose()
	})

	it('the icon stays while the body grows, one per block', () => {
		const doc = new MarkdownDocument(OPTIONS)
		let html = ''
		for (const source of ['```rust\nfn', '```rust\nfn main() {', '```rust\nfn main() {}\nlet x = 1;\n']) {
			html = htmlOf(doc.update(source, false))
			expect(icons(html)).toHaveLength(1)
		}
		expect(html).toContain('let x')
		doc.dispose()
	})

	it('the one-shot rendering of the closed fence has the same icon', () => {
		const html = htmlOf(renderReference('```rust\nfn main() {}\n```\n', OPTIONS))
		expect(icons(html)).toHaveLength(1)
		expect(srcOf(icons(html)[0]!)).toMatch(/file-icons\/svg\/rust\.svg$/)
	})

	it('the copy button copies the code only, not the header or the icon', async () => {
		const writeText = vi.fn(async (_text: string) => undefined)
		Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true })
		// The frame of a block as it is written (not sanitized: the copy reads the header's code only).
		const container = document.createElement('div')
		container.innerHTML = codeShell('rust', 'fn main() {}', 'rust')
		document.body.append(container)
		markdownClicks(container)
		container.querySelector<HTMLButtonElement>('button[data-copy]')!.click()
		await vi.waitFor(() => expect(writeText).toHaveBeenCalledTimes(1))
		const copied = writeText.mock.calls[0]![0]
		expect(copied).toContain('fn main() {}')
		expect(copied).not.toContain('rust')
		expect(copied).not.toContain('<img')
		container.remove()
	})
})

describe('the icon comes from the file the fence names, or from its language', () => {
	it('a path in the info string wins over the language', () => {
		expect(fenceIconId('ts src/app.py')).toBe('python')
		const html = htmlOf(renderReference('```ts src/app.py\nprint(1)\n```\n', OPTIONS))
		expect(srcOf(icons(html)[0]!)).toMatch(/file-icons\/svg\/python\.svg$/)
	})

	it('a bare language names its icon', () => {
		expect(fenceIconId('typescript')).toBe('typescript')
		expect(fenceIconId('rs')).toBe('rust')
	})

	it('an unknown language has no icon: no image in its header', () => {
		expect(fenceIconId('unknownlang')).toBeNull()
		const html = htmlOf(renderReference('```unknownlang\nfoo\n```\n', OPTIONS))
		expect(icons(html)).toEqual([])
		expect(html).toContain('unknownlang')
	})
})

describe('a model cannot make an image with its own HTML', () => {
	it('an inline <img> with a handler is shown as text, never as an image', () => {
		const html = htmlOf(renderReference('Texte <img src=x onerror=alert(1)> fin', OPTIONS))
		expect(icons(html)).toEqual([])
		// The handler is text now: no tag carries it.
		expect(html).not.toMatch(/<img/i)
		expect(html).not.toMatch(/<[^>]*onerror/i)
		expect(html).toContain('&lt;img src=x onerror=alert(1)&gt;')
	})

	it('a block <img> with a handler is removed by the sanitizer', () => {
		const html = htmlOf(renderReference('<img src=x onerror=alert(1)>\n\nfin', OPTIONS))
		expect(html).not.toMatch(/<img/i)
		expect(html).not.toMatch(/onerror/i)
		expect(html).toContain('fin')
	})

	it('a literal code-icon mark in a paragraph is shown as text, not as an image', () => {
		const html = htmlOf(renderReference('Texte <span data-code-icon="rust"></span> fin', OPTIONS))
		expect(icons(html)).toEqual([])
		expect(html).toContain('&lt;span data-code-icon=')
	})

	it('a literal code-icon mark at the start of a line is shown as text too', () => {
		const html = htmlOf(renderReference('<span data-code-icon="rust"></span>\n\nfin', OPTIONS))
		expect(icons(html)).toEqual([])
	})

	it('withCodeIcons turns only the marks codeShell signed into images', () => {
		expect(icons(withCodeIcons(codeShell('rust', 'x', 'rust')))).toHaveLength(1)
		// An unsigned mark (written by a message) stays an inert span: no image.
		for (const id of ['rust', 'not-an-id', '../../etc', 'constructor']) {
			const mark = `<span data-code-icon="${id}"></span>`
			expect(icons(withCodeIcons(mark))).toEqual([])
		}
		// A forged key is not the renderer's.
		expect(icons(withCodeIcons('<span data-code-icon="rust" data-code-icon-key="0000"></span>'))).toEqual([])
	})

	it('codeShell writes a signed mark only for a catalog id', () => {
		expect(codeShell('rust', 'x', 'rust')).toMatch(/<span data-code-icon="rust" data-code-icon-key="[0-9a-f]{32}"><\/span>/)
		expect(codeShell('rust', 'x', 'not-an-id')).not.toContain('data-code-icon')
		expect(codeShell('rust', 'x', 'constructor')).not.toContain('data-code-icon')
		expect(codeShell('rust', 'x', null)).not.toContain('data-code-icon')
		expect(isIconId('rust')).toBe(true)
		expect(isIconId('constructor')).toBe(false)
	})

	it('a label is escaped in the header: markup in it stays text', () => {
		const shell = codeShell('<img src=x onerror=alert(1)>', 'x', 'rust')
		expect(shell).not.toMatch(/<img src=x/)
		const rendered = withCodeIcons(shell)
		expect(icons(rendered)).toHaveLength(1)
		expect(rendered).not.toMatch(/<[^>]*onerror/i)
		expect(rendered).toContain('&lt;img src=x onerror=alert(1)&gt;')
	})

	it('the sanitizer keeps no img at all in finished markdown', () => {
		expect(sanitizeMarkdown('<img src=x onerror=alert(1)>')).not.toMatch(/<img/i)
	})

	it('a code-icon mark inside model-written block HTML must not become an image (actual: an <img class="file-icon"> is produced)', () => {
		// The sanitizer keeps `<span data-code-icon>` (data attributes are allowed) and withCodeIcons
		// then turns the mark into an image. Reproduced on finishHtml, which the final render
		// relies on: sanitizeMarkdown keeps the span of this paragraph.
		const html = finishHtml('<p><span data-code-icon="rust"></span> x</p>', [], NO_MATH)
		expect(icons(html)).toEqual([])
	})
})

describe('Mermaid and diff fences are unchanged', () => {
	it('a mermaid fence is a mermaid block, a diff fence a diff block, and neither has a header icon', () => {
		const blocks = renderReference('```mermaid\ngraph TD; A-->B\n```\n\n```diff\n-a\n+b\n```\n', OPTIONS)
		expect(blocks.map((block) => block.kind)).toEqual(['mermaid', 'diff'])
		expect(blocks[0]).toMatchObject({ kind: 'mermaid', source: 'graph TD; A-->B', complete: true })
		expect(blocks[1]).toMatchObject({ kind: 'diff', source: '-a\n+b', complete: true })
		expect(icons(htmlOf(blocks))).toEqual([])
	})

	it('an open mermaid fence while streaming stays a mermaid block', () => {
		const doc = new MarkdownDocument(OPTIONS)
		const blocks = doc.update('```mermaid\ngraph TD; A-->', false)
		expect(blocks).toHaveLength(1)
		expect(blocks[0]).toMatchObject({ kind: 'mermaid', complete: false })
		doc.dispose()
	})
})
