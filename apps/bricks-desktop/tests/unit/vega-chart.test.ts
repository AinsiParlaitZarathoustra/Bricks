// @vitest-environment happy-dom
// vega.ts: the parts of a ```vega-lite fence. Pure logic first (parse, merge, theme,
// width, height), then the real Vega and Vega-Lite engines under happy-dom: a chart is
// drawn to an SVG that the sanitizer keeps, external and text data are refused,
// expressions run through the interpreter, and no global is reached. The Worker
// protocol (timeout, stop, sanitizing of the answer) is checked with a fake Worker.
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

// happy-dom answers '' to `Node.prototype.nodeName` for non-element nodes; DOMPurify
// (used by sanitizeSvg) reads it through that getter. Same shim as markdown-sanitize.test.ts.
const nodeNameFallback = Object.getOwnPropertyDescriptor(Node.prototype, 'nodeName')
Object.defineProperty(Node.prototype, 'nodeName', {
	configurable: true,
	get(this: Node) {
		switch (this.nodeType) {
			case Node.ELEMENT_NODE:
				return (this as Element).tagName
			case Node.TEXT_NODE:
				return '#text'
			case Node.CDATA_SECTION_NODE:
				return '#cdata-section'
			case Node.PROCESSING_INSTRUCTION_NODE:
				return (this as ProcessingInstruction).target
			case Node.COMMENT_NODE:
				return '#comment'
			case Node.DOCUMENT_NODE:
				return '#document'
			case Node.DOCUMENT_FRAGMENT_NODE:
				return '#document-fragment'
			default:
				return nodeNameFallback?.get?.call(this) ?? ''
		}
	}
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
		detach() {}
	}
}) as unknown as Document['createNodeIterator']

const { sanitizeSvg } = await import('../../src/lib/features/conversation/markdown/sanitize')
const {
	MAX_VEGA_SOURCE,
	VEGA_TIMEOUT_MS,
	externalResource,
	mergeConfig,
	parseVegaLite,
	prepareSpec,
	renderVegaLite,
	themeConfig,
	textData
} = await import('../../src/lib/features/conversation/markdown/vega')

const COLORS = {
	foreground: '#111111',
	muted: '#666666',
	border: '#dddddd',
	primary: '#0000ff',
	chart1: '#101010',
	chart2: '#202020',
	chart3: '#303030',
	chart4: '#404040',
	chart5: '#505050'
}
const RENDER = { width: 480, colors: COLORS, fontFamily: 'Inter, sans-serif' }

const POINTS = {
	data: {
		values: [
			{ x: 1, y: 2, k: 'a' },
			{ x: 2, y: 5, k: 'b' },
			{ x: 3, y: 4, k: 'a' },
			{ x: 4, y: 8, k: 'b' }
		]
	}
}

function spec(extra: Record<string, unknown> = {}): string {
	return JSON.stringify({
		...POINTS,
		mark: 'point',
		encoding: {
			x: { field: 'x', type: 'quantitative' },
			y: { field: 'y', type: 'quantitative' }
		},
		...extra
	})
}

function svgOf(result: Awaited<ReturnType<typeof renderVegaLite>>): string {
	if (!('svg' in result)) throw new Error(`pas de SVG : ${result.error}`)
	return result.svg
}

describe('externalResource', () => {
	it('finds a string url at the root, nested in objects, and nested in arrays', () => {
		expect(externalResource({ url: 'https://x' } as never)).toBe('url')
		expect(externalResource({ data: { url: 'https://x' } } as never)).toBe('data.url')
		expect(externalResource({ layer: [{ mark: 'line' }, { data: { url: 'a.csv' } }] } as never)).toBe('layer[1].data.url')
	})

	it('ignores inline rows, whatever their columns are named', () => {
		expect(externalResource({ data: { values: [{ url: 'https://x' }] } } as never)).toBeNull()
		expect(externalResource({ layer: [{ data: { values: [{ url: 'a' }] } }] } as never)).toBeNull()
	})

	it('still refuses a url beside inline values of the same data', () => {
		expect(externalResource({ data: { values: [{ url: 'ok' }], url: 'https://evil' } } as never)).toBe('data.url')
	})

	it('returns null for a spec without any url', () => {
		expect(externalResource(JSON.parse(spec()) as never)).toBeNull()
	})
})

describe('parseVegaLite', () => {
	it('accepts a JSON object specification', () => {
		const parsed = parseVegaLite(spec())
		expect(parsed).toHaveProperty('spec')
	})

	it('refuses text that is not JSON', () => {
		const parsed = parseVegaLite('{"mark": point}')
		expect(parsed).toEqual({ error: expect.stringMatching(/^JSON invalide :/) })
	})

	it('refuses an array, a scalar and null: a chart is an object', () => {
		for (const text of ['[{"mark":"point"}]', '"point"', '42', 'null', 'true']) {
			expect(parseVegaLite(text)).toEqual({ error: 'La spécification doit être un objet JSON' })
		}
	})

	it('accepts a specification of exactly MAX_VEGA_SOURCE characters and refuses one more', () => {
		const head = '{"d":"'
		const tail = '"}'
		const exact = head + 'x'.repeat(MAX_VEGA_SOURCE - head.length - tail.length) + tail
		expect(exact.length).toBe(MAX_VEGA_SOURCE)
		expect(parseVegaLite(exact)).toHaveProperty('spec')
		const over = head + 'x'.repeat(MAX_VEGA_SOURCE - head.length - tail.length + 1) + tail
		expect(parseVegaLite(over)).toEqual({ error: expect.stringContaining('trop longue') })
	})

	it('refuses a data.url and names the path', () => {
		expect(parseVegaLite(JSON.stringify({ data: { url: 'https://example.com/a.csv' }, mark: 'point' }))).toEqual({
			error: expect.stringContaining('data.url')
		})
	})

	it('refuses a url nested in a layer, and names it', () => {
		const parsed = parseVegaLite(JSON.stringify({ layer: [{ mark: 'line' }, { data: { url: 'x.json' } }] }))
		expect(parsed).toEqual({ error: expect.stringContaining('layer[1].data.url') })
	})

	it('accepts a url-named column inside inline values', () => {
		const parsed = parseVegaLite(JSON.stringify({ data: { values: [{ url: 'https://example.com', n: 1 }] }, mark: 'text', encoding: { text: { field: 'url' } } }))
		expect(parsed).toHaveProperty('spec')
	})
})

describe('mergeConfig', () => {
	it('lets the chart`s own config win over the base, key by key, deeply', () => {
		const base = { a: 1, b: { c: 1, d: 2 }, e: 'keep' }
		expect(mergeConfig(base, { b: { c: 9 }, a: 3 })).toEqual({ a: 3, b: { c: 9, d: 2 }, e: 'keep' })
	})

	it('replaces arrays and sets null values', () => {
		expect(mergeConfig({ x: [1, 2], y: 'v' }, { x: [3], y: null })).toEqual({ x: [3], y: null })
	})

	it('returns the base unchanged when the chart gives no object', () => {
		const base = { a: 1 }
		expect(mergeConfig(base, undefined)).toBe(base)
		expect(mergeConfig(base, [1, 2])).toBe(base)
		expect(mergeConfig(base, 'x')).toBe(base)
	})

	it('does not modify the base nor the override', () => {
		const base = { b: { c: 1 } }
		const over = { b: { d: 2 } }
		mergeConfig(base, over)
		expect(base).toEqual({ b: { c: 1 } })
		expect(over).toEqual({ b: { d: 2 } })
	})
})

describe('themeConfig', () => {
	it('starts the series from chart 2, then chart 1, then the rest and the primary colour', () => {
		const config = themeConfig(COLORS, 'Inter') as { range: { category: string[] }; mark: { color: string } }
		expect(config.range.category).toEqual(['#202020', '#101010', '#303030', '#404040', '#505050', '#0000ff'])
		expect(config.mark.color).toBe('#202020')
	})

	it('carries the text, axis and grid colours and the font', () => {
		const config = themeConfig(COLORS, 'Inter') as Record<string, Record<string, unknown>>
		expect(config.font).toBe('Inter')
		expect(config.background).toBeNull()
		expect(config.axis).toMatchObject({ labelColor: '#666666', titleColor: '#111111', gridColor: '#dddddd' })
		expect(config.title).toMatchObject({ color: '#111111', subtitleColor: '#666666' })
	})

	it('falls back to the foreground for the muted text, and omits the series without colours', () => {
		const config = themeConfig({ foreground: '#123456' }, 'sans-serif') as Record<string, unknown>
		expect(config.axis).toMatchObject({ labelColor: '#123456' })
		expect(config).not.toHaveProperty('range')
		expect(config).not.toHaveProperty('mark')
	})
})

describe('prepareSpec', () => {
	const base = JSON.parse(spec()) as Parameters<typeof prepareSpec>[0]

	it('takes the available width when the chart asks for none', () => {
		const out = prepareSpec(base, 500, COLORS, 'Inter')
		expect(out.width).toBe(500)
		expect(out.autosize).toEqual({ type: 'fit', contains: 'padding' })
	})

	it('takes the available width for "container", and keeps an explicit width', () => {
		expect(prepareSpec({ ...base, width: 'container' }, 500, COLORS, 'Inter').width).toBe(500)
		const fixed = prepareSpec({ ...base, width: 300 }, 500, COLORS, 'Inter')
		expect(fixed.width).toBe(300)
		expect(fixed).not.toHaveProperty('autosize')
	})

	it('never draws narrower than 120 pixels', () => {
		expect(prepareSpec(base, 50, COLORS, 'Inter').width).toBe(120)
	})

	it('keeps the width absent when the container has none yet', () => {
		expect(prepareSpec(base, 0, COLORS, 'Inter')).not.toHaveProperty('width')
	})

	it('leaves composite views (facet, concat, repeat, layouts) at their own width', () => {
		for (const key of ['facet', 'hconcat', 'vconcat', 'concat', 'repeat']) {
			const out = prepareSpec({ ...base, [key]: [] }, 500, COLORS, 'Inter')
			expect(out).not.toHaveProperty('width')
		}
	})

	it('applies the app theme under the chart`s own config', () => {
		const out = prepareSpec({ ...base, config: { mark: { color: '#ff0000' } } }, 500, COLORS, 'Inter')
		expect(out.config).toMatchObject({ mark: { color: '#ff0000' }, font: 'Inter', background: null })
	})

	it('does not modify the specification it is given', () => {
		const copy = JSON.stringify(base)
		prepareSpec(base, 500, COLORS, 'Inter')
		expect(JSON.stringify(base)).toBe(copy)
	})
})

describe('renderVegaLite', () => {
	it('draws a scatter plot as an SVG holding its points', async () => {
		const svg = svgOf(await renderVegaLite(spec(), RENDER))
		expect(svg).toMatch(/^<svg[\s>]/)
		expect(svg).toContain('</svg>')
		expect(svg.match(/<path[\s>]/g)?.length ?? 0).toBeGreaterThanOrEqual(POINTS.data.values.length)
	})

	it('draws a line chart', async () => {
		const svg = svgOf(await renderVegaLite(spec({ mark: 'line' }), RENDER))
		expect(svg).toContain('<path')
	})

	it('draws a logarithmic scale and labels its decades', async () => {
		const svg = svgOf(
			await renderVegaLite(
				spec({
					encoding: {
						x: { field: 'x', type: 'quantitative' },
						y: { field: 'y', type: 'quantitative', scale: { type: 'log' } }
					}
				}),
				RENDER
			)
		)
		expect(svg).toMatch(/>10</)
	})

	it('keeps the explicit colours of a colour encoding in the SVG', async () => {
		const svg = svgOf(
			await renderVegaLite(
				spec({
					encoding: {
						x: { field: 'x', type: 'quantitative' },
						y: { field: 'y', type: 'quantitative' },
						color: { field: 'k', type: 'nominal', scale: { range: ['#ff0000', '#00ff00'] } }
					}
				}),
				RENDER
			)
		)
		expect(svg.toLowerCase()).toContain('#ff0000')
		expect(svg.toLowerCase()).toContain('#00ff00')
	})

	it('keeps a legend for a colour encoding', async () => {
		const svg = svgOf(
			await renderVegaLite(
				spec({
					encoding: {
						x: { field: 'x', type: 'quantitative' },
						y: { field: 'y', type: 'quantitative' },
						color: { field: 'k', type: 'nominal' }
					}
				}),
				RENDER
			)
		)
		expect(svg).toContain('>a<')
		expect(svg).toContain('>b<')
	})

	it('draws the width it was given', async () => {
		const svg = svgOf(await renderVegaLite(spec(), { ...RENDER, width: 360 }))
		expect(svg).toMatch(/<svg[^>]*\bwidth="360"/)
	})

	it('refuses data.url before anything is loaded', async () => {
		const result = await renderVegaLite(JSON.stringify({ data: { url: 'https://example.com/a.csv' }, mark: 'point' }), RENDER)
		expect(result).toEqual({ error: expect.stringContaining('Ressource externe refusée (data.url)') })
	})

	it('refuses an image mark that would fetch an address, and never shows that address', async () => {
		const result = await renderVegaLite(
			JSON.stringify({
				data: { values: [{ u: 'https://example.com/pixel.png', x: 1, y: 1 }] },
				mark: { type: 'image', width: 20, height: 20 },
				encoding: {
					x: { field: 'x', type: 'quantitative' },
					y: { field: 'y', type: 'quantitative' },
					url: { field: 'u', type: 'nominal' }
				}
			}),
			RENDER
		)
		if ('svg' in result) expect(result.svg).not.toContain('pixel.png')
		else expect(result.error).toBeTruthy()
	})

	it('evaluates a Vega expression through the interpreter: y2 is x times two', async () => {
		const svg = svgOf(
			await renderVegaLite(
				JSON.stringify({
					data: { values: [{ x: 1 }, { x: 3 }, { x: 5 }] },
					transform: [{ calculate: 'datum.x * 2', as: 'y2' }],
					mark: 'text',
					encoding: {
						x: { field: 'x', type: 'quantitative' },
						y: { value: 10 },
						text: { field: 'y2', type: 'quantitative' }
					}
				}),
				RENDER
			)
		)
		expect(svg).toMatch(/>2</)
		expect(svg).toMatch(/>6</)
		expect(svg).toMatch(/>10</)
	})

	it('does not run script carried by an expression: no global is set', async () => {
		const marker = '__bricksVegaProbe'
		;(globalThis as Record<string, unknown>)[marker] = undefined
		const result = await renderVegaLite(
			JSON.stringify({
				data: { values: [{ x: 1 }] },
				transform: [{ calculate: `(globalThis.${marker} = 1) + datum.x`, as: 'z' }],
				mark: 'point',
				encoding: { x: { field: 'x', type: 'quantitative' }, y: { field: 'z', type: 'quantitative' } }
			}),
			RENDER
		)
		expect((globalThis as Record<string, unknown>)[marker]).toBeUndefined()
		expect(result).toBeDefined()
		delete (globalThis as Record<string, unknown>)[marker]
	})

	it('reports an invalid specification as an error, not a drawing', async () => {
		const result = await renderVegaLite(JSON.stringify({ mark: 'nonexistent-mark', data: { values: [] } }), RENDER)
		expect(result).toEqual({ error: expect.stringMatching(/^(Spécification invalide|Graphique non rendu) :/) })
	})

	it('reports text that is not JSON without loading the engines', async () => {
		expect(await renderVegaLite('{ nope', RENDER)).toEqual({ error: expect.stringMatching(/^JSON invalide :/) })
	})

	it('returns an SVG that the sanitizer leaves unchanged in its markup', async () => {
		const svg = svgOf(await renderVegaLite(spec(), RENDER))
		const clean = sanitizeSvg(svg)
		expect(clean).toMatch(/^<svg[\s>]/)
		expect(clean).not.toMatch(/<script|<foreignObject|<iframe|<image\b/i)
		expect(clean).not.toMatch(/\son[a-z]+\s*=/i)
		expect(clean).not.toMatch(/href\s*=\s*["']?\s*javascript:/i)
	})

	it('draws the example chart of the system prompt FIGURES section', async () => {
		// npm run test:unit runs from apps/bricks-desktop: the crates are two folders up.
		const prompt = readFileSync(resolve(process.cwd(), '../../crates/bricks-agent/src/system_prompt.rs'), 'utf8')
		const block = /```vega-lite\n([\s\S]*?)\n```/.exec(prompt)?.[1]
		expect(block, 'the FIGURES example is in system_prompt.rs').toBeTruthy()
		const svg = svgOf(await renderVegaLite(block ?? '', RENDER))
		expect(svg).toMatch(/<svg[\s>]/)
		expect(svg).toContain('<path')
	})
})

describe('textData: inline text data is refused', () => {
	it('finds csv, tsv and dsv formats, at the root and in a layer', () => {
		expect(textData({ data: { format: { type: 'csv' } } } as never)).toBe('data.format')
		expect(textData({ data: { format: { type: 'tsv' } } } as never)).toBe('data.format')
		expect(textData({ data: { format: { type: 'dsv', delimiter: ';' } } } as never)).toBe('data.format')
		expect(textData({ layer: [{ data: { format: { type: 'csv' } } }] } as never)).toBe('layer[0].data.format')
		expect(textData({ layer: [{ mark: 'line' }, { data: { format: { type: 'tsv' } } }] } as never)).toBe('layer[1].data.format')
	})

	it('accepts a json format and a format without a type', () => {
		expect(textData({ data: { format: { type: 'json' }, values: [{ x: 1 }] } } as never)).toBeNull()
		expect(textData({ data: { format: { property: 'rows' } } } as never)).toBeNull()
	})

	it('refuses a string given as inline values, at any depth', () => {
		expect(textData({ data: { values: 'x,y\n1,2' } } as never)).toBe('data.values')
		expect(textData({ layer: [{ data: { values: 'a' } }] } as never)).toBe('layer[0].data.values')
	})

	it('ignores a format key inside inline rows', () => {
		expect(textData({ data: { values: [{ format: { type: 'csv' } }] } } as never)).toBeNull()
	})

	it('is refused by parseVegaLite, with the path and the advice', () => {
		expect(parseVegaLite(JSON.stringify({ data: { format: { type: 'csv' } }, mark: 'point' }))).toEqual({
			error: 'Données texte refusées (data.format) : donne les lignes en objets JSON dans « values »'
		})
		expect(parseVegaLite(JSON.stringify({ data: { values: [{ x: 1 }], format: { type: 'tsv' } }, mark: 'point' }))).toEqual({
			error: expect.stringContaining('Données texte refusées (data.format)')
		})
		expect(parseVegaLite(JSON.stringify({ layer: [{ mark: 'line' }, { data: { format: { type: 'csv' } } }] }))).toEqual({
			error: expect.stringContaining('Données texte refusées (layer[1].data.format)')
		})
		expect(parseVegaLite(JSON.stringify({ data: { values: 'a,b' }, mark: 'point' }))).toEqual({
			error: expect.stringContaining('Données texte refusées (data.values)')
		})
	})

	it('reports an external url before inline text', () => {
		expect(parseVegaLite(JSON.stringify({ data: { url: 'https://example.com/a.csv', format: { type: 'csv' } } }))).toEqual({
			error: expect.stringContaining('Ressource externe refusée (data.url)')
		})
	})
})

describe('datasets at the root of a specification', () => {
	const withDatasets = {
		datasets: { points: [{ url: 'https://example.com/a', n: 1 }] },
		data: { name: 'points' },
		mark: 'text',
		encoding: { text: { field: 'url', type: 'nominal' } }
	}

	it('ignores a column named url in a top-level dataset', () => {
		expect(externalResource(withDatasets as never)).toBeNull()
		expect(parseVegaLite(JSON.stringify(withDatasets))).toHaveProperty('spec')
	})

	it('ignores a format key in a top-level dataset row', () => {
		expect(textData({ datasets: { points: [{ format: { type: 'csv' } }] } } as never)).toBeNull()
	})

	it('refuses a url nested anywhere else, beside top-level datasets', () => {
		expect(externalResource({ ...withDatasets, layer: [{ datasets: { y: [{ url: 'https://evil' }] } }] } as never)).toBe(
			'layer[0].datasets.y[0].url'
		)
		expect(externalResource({ ...withDatasets, data: { name: 'points', url: 'https://evil' } } as never)).toBe('data.url')
	})

	it('refuses a csv format nested in a layer, beside top-level datasets', () => {
		expect(textData({ ...withDatasets, layer: [{ data: { format: { type: 'csv' } } }] } as never)).toBe('layer[0].data.format')
	})
})

describe('prepareSpec height', () => {
	const base = JSON.parse(spec()) as Parameters<typeof prepareSpec>[0]

	it('removes a "container" height, so the chart keeps its own', () => {
		expect(prepareSpec({ ...base, height: 'container' }, 500, COLORS, 'Inter')).not.toHaveProperty('height')
	})

	it('keeps a numeric height', () => {
		expect(prepareSpec({ ...base, height: 240 }, 500, COLORS, 'Inter').height).toBe(240)
	})

	it('adds no height when the chart has none', () => {
		expect(prepareSpec(base, 500, COLORS, 'Inter')).not.toHaveProperty('height')
	})
})

describe('renderVegaLite in the page, without a Worker', () => {
	it('has no Worker here, so the chart is drawn in the page', async () => {
		expect(typeof Worker).toBe('undefined')
		expect(svgOf(await renderVegaLite(spec(), RENDER))).toMatch(/^<svg[\s>]/)
	})

	it('reports a specification Vega-Lite cannot compile with the prefix', async () => {
		const result = await renderVegaLite(JSON.stringify({ mark: 'nonexistent-mark', data: { values: [] } }), RENDER)
		expect(result).toEqual({ error: expect.stringMatching(/^Spécification invalide : /) })
	})

	it('never throws, whatever the text', async () => {
		const texts = [
			'',
			'{',
			'[]',
			'{"layer":[]}',
			'{"mark":"point","encoding":{"x":{"field":"x","type":"bogus"}}}',
			'{"mark":"point","data":{"values":[{"x":1}]},"transform":[{"calculate":"(","as":"z"}]}'
		]
		for (const text of texts) {
			const result = await renderVegaLite(text, RENDER)
			expect('svg' in result || 'error' in result).toBe(true)
		}
	})
})

/** A Worker that never answers, as a test double; `terminate` is a spy. */
class SilentWorker {
	static instances: SilentWorker[] = []
	onmessage: ((event: { data: unknown }) => void) | null = null
	onerror: ((event: { message: string; preventDefault: () => void }) => void) | null = null
	posted: Array<{ id: number; spec: Record<string, unknown> }> = []
	terminate = vi.fn()

	constructor() {
		SilentWorker.instances.push(this)
	}

	postMessage(message: { id: number; spec: Record<string, unknown> }) {
		this.posted.push(message)
	}
}

const TIMEOUT_ERROR = { error: 'rendu trop long (plus de 10 s)' }

describe('renderVegaLite with a Worker', () => {
	let engine: typeof import('../../src/lib/features/conversation/markdown/vega')

	beforeEach(async () => {
		vi.resetModules()
		SilentWorker.instances = []
		vi.stubGlobal('Worker', SilentWorker)
		engine = await import('../../src/lib/features/conversation/markdown/vega')
	})

	afterEach(() => {
		vi.useRealTimers()
		vi.unstubAllGlobals()
	})

	it('gives up a chart that never answers after VEGA_TIMEOUT_MS, and stops its worker', async () => {
		vi.useFakeTimers()
		const pending = engine.renderVegaLite(spec(), RENDER)
		const worker = SilentWorker.instances[0]
		await vi.advanceTimersByTimeAsync(VEGA_TIMEOUT_MS - 1)
		expect(worker?.terminate).not.toHaveBeenCalled()
		await vi.advanceTimersByTimeAsync(1)
		expect(await pending).toEqual(TIMEOUT_ERROR)
		expect(worker?.terminate).toHaveBeenCalledTimes(1)
	})

	it('answers every chart that waits on the stopped worker', async () => {
		vi.useFakeTimers()
		const first = engine.renderVegaLite(spec(), RENDER)
		const second = engine.renderVegaLite(spec({ mark: 'line' }), RENDER)
		expect(SilentWorker.instances).toHaveLength(1)
		await vi.advanceTimersByTimeAsync(VEGA_TIMEOUT_MS)
		expect(await first).toEqual(TIMEOUT_ERROR)
		expect(await second).toEqual(TIMEOUT_ERROR)
		expect(SilentWorker.instances[0]?.terminate).toHaveBeenCalledTimes(1)
	})

	it('creates a new worker for the next chart after a stop', async () => {
		vi.useFakeTimers()
		const first = engine.renderVegaLite(spec(), RENDER)
		await vi.advanceTimersByTimeAsync(VEGA_TIMEOUT_MS)
		await first
		const second = engine.renderVegaLite(spec(), RENDER)
		expect(SilentWorker.instances).toHaveLength(2)
		await vi.advanceTimersByTimeAsync(VEGA_TIMEOUT_MS)
		expect(await second).toEqual(TIMEOUT_ERROR)
	})

	it('sends the prepared specification, and returns the sanitized SVG of the answer', async () => {
		const pending = engine.renderVegaLite(spec(), RENDER)
		const worker = SilentWorker.instances[0]
		const sent = worker?.posted[0]
		expect(sent?.spec).toMatchObject({ width: 480, config: { font: 'Inter, sans-serif' } })
		worker?.onmessage?.({
			data: {
				id: sent?.id,
				svg: '<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10" onload="alert(1)"><rect width="5" height="5" onclick="x()"/></svg>'
			}
		})
		const result = await pending
		expect(result).toHaveProperty('svg')
		const svg = 'svg' in result ? result.svg : ''
		expect(svg).toMatch(/^<svg[\s>]/)
		expect(svg).toContain('<rect')
		expect(svg).not.toMatch(/onload|onclick|alert/i)
		expect(SilentWorker.instances).toHaveLength(1)
		expect(worker?.terminate).not.toHaveBeenCalled()
	})

	it('removes a script from the answer of the worker', async () => {
		const pending = engine.renderVegaLite(spec(), RENDER)
		const worker = SilentWorker.instances[0]
		worker?.onmessage?.({
			data: { id: worker.posted[0]?.id, svg: '<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><script>alert(2)</script></svg>' }
		})
		const result = await pending
		expect(result).toHaveProperty('svg')
		const svg = 'svg' in result ? result.svg : ''
		expect(svg).toMatch(/^<svg[\s>]/)
		expect(svg).not.toMatch(/<script|alert/i)
	})

	it('shows no chart when the environment is neither a test nor a worker', async () => {
		vi.stubEnv('MODE', 'production')
		try {
			vi.stubGlobal('Worker', undefined)
			expect(await engine.renderVegaLite(spec(), RENDER)).toEqual({ error: 'Graphiques indisponibles ici' })
		} finally {
			vi.unstubAllEnvs()
		}
	})

	it('reports a specification the worker could not compile, with the prefix', async () => {
		const pending = engine.renderVegaLite(spec(), RENDER)
		const worker = SilentWorker.instances[0]
		worker?.onmessage?.({ data: { id: worker.posted[0]?.id, error: 'Invalid mark', spec: true } })
		expect(await pending).toEqual({ error: 'Spécification invalide : Invalid mark' })
	})

	it('reports another drawing error without the prefix', async () => {
		const pending = engine.renderVegaLite(spec(), RENDER)
		const worker = SilentWorker.instances[0]
		worker?.onmessage?.({ data: { id: worker.posted[0]?.id, error: 'boom' } })
		expect(await pending).toEqual({ error: 'boom' })
	})

	it('reports an empty drawing', async () => {
		const pending = engine.renderVegaLite(spec(), RENDER)
		const worker = SilentWorker.instances[0]
		worker?.onmessage?.({ data: { id: worker.posted[0]?.id, svg: '' } })
		expect(await pending).toEqual({ error: 'Rendu vide' })
	})

	it('reports a worker that fails to load, and stops it', async () => {
		const pending = engine.renderVegaLite(spec(), RENDER)
		const worker = SilentWorker.instances[0]
		const preventDefault = vi.fn()
		worker?.onerror?.({ message: 'Script error', preventDefault })
		expect(preventDefault).toHaveBeenCalled()
		expect(await pending).toEqual({ error: "Vega n'a pas pu être chargé : Script error" })
		expect(worker?.terminate).toHaveBeenCalledTimes(1)
	})
})

describe('vega-draw: the drawing of a prepared specification', () => {
	it('draws a simple chart as an SVG string', async () => {
		const { drawVegaLite } = await import('../../src/lib/features/conversation/markdown/vega-draw')
		const svg = await drawVegaLite(JSON.parse(spec()))
		expect(svg).toMatch(/^<svg[\s>]/)
		expect(svg).toContain('<path')
	})

	it('rejects with a SpecError for a specification Vega-Lite does not compile', async () => {
		const { SpecError, drawVegaLite } = await import('../../src/lib/features/conversation/markdown/vega-draw')
		const error: unknown = await drawVegaLite({ mark: 'nonexistent-mark', data: { values: [] } }).catch((e: unknown) => e)
		expect(error).toBeInstanceOf(SpecError)
	})

	it('does not load external data: the refused load draws no point', async () => {
		const { drawVegaLite } = await import('../../src/lib/features/conversation/markdown/vega-draw')
		const svg = await drawVegaLite({
			data: { url: 'https://example.com/a.csv' },
			mark: 'point',
			encoding: { x: { field: 'x', type: 'quantitative' } }
		})
		expect(svg).toMatch(/^<svg[\s>]/)
		expect(svg).toContain('aria-roledescription="symbol mark container"/>')
	})
})

describe('vega.worker: the message protocol', () => {
	type Handler = (event: { data: { id: number; spec: object } }) => Promise<void>

	async function answer(message: { id: number; spec: object }): Promise<unknown[]> {
		await import('../../src/lib/features/conversation/markdown/vega.worker')
		const scope = self as unknown as { onmessage: Handler }
		const post = vi.spyOn(self, 'postMessage').mockImplementation(() => {})
		try {
			await scope.onmessage({ data: message })
			return post.mock.calls.map((call) => call[0])
		} finally {
			post.mockRestore()
		}
	}

	it('answers a drawn chart with its id and its SVG, not yet sanitized', async () => {
		const [reply] = await answer({ id: 7, spec: JSON.parse(spec()) })
		expect(reply).toMatchObject({ id: 7, svg: expect.stringMatching(/^<svg[\s>]/) })
	})

	it('answers a specification it cannot compile with its id, the message and the spec flag', async () => {
		const [reply] = await answer({ id: 8, spec: { mark: 'nonexistent-mark', data: { values: [] } } })
		expect(reply).toEqual({ id: 8, error: expect.any(String), spec: true })
	})
})
