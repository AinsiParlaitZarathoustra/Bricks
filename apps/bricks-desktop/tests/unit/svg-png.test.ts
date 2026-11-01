// @vitest-environment happy-dom
// svg-png.ts: the PNG of a diagram. Sizes and scales are pure; svgToPng is tested with
// the browser's image decoder and canvas replaced by fakes, so that the temporary URL,
// the background and the failure paths are checked without a real rasteriser.
import { afterEach, describe, expect, it, vi } from 'vitest'
import {
	FALLBACK_SIZE,
	MAX_SIDE,
	PngError,
	TARGET_SCALE,
	canvasSize,
	effectiveBackground,
	fontFamilies,
	pngScale,
	pngSize,
	standaloneSvg,
	svgToPng
} from '../../src/lib/features/conversation/markdown/svg-png'

describe('pngSize', () => {
	it('uses the drawing size when both sides are positive and finite', () => {
		expect(pngSize({ width: 300, height: 200 })).toEqual({ width: 300, height: 200 })
	})

	it('falls back to 1200 x 800 without a drawing, or with a side that is not a real size', () => {
		const fallback = { width: 1200, height: 800 }
		expect(FALLBACK_SIZE).toEqual(fallback)
		expect(pngSize(null)).toEqual(fallback)
		expect(pngSize(undefined)).toEqual(fallback)
		expect(pngSize({ width: 0, height: 10 })).toEqual(fallback)
		expect(pngSize({ width: -5, height: 10 })).toEqual(fallback)
		expect(pngSize({ width: Number.NaN, height: 10 })).toEqual(fallback)
		expect(pngSize({ width: 10, height: Number.POSITIVE_INFINITY })).toEqual(fallback)
	})

	it('returns a copy, not the object it was given', () => {
		const given = { width: 40, height: 30 }
		const out = pngSize(given)
		expect(out).not.toBe(given)
		out.width = 1
		expect(given.width).toBe(40)
	})
})

describe('pngScale', () => {
	it('is the target scale for a small drawing', () => {
		expect(pngScale({ width: 100, height: 50 })).toBe(TARGET_SCALE)
		expect(TARGET_SCALE).toBe(2)
	})

	it('is the target scale exactly when the longest side meets the limit at that scale', () => {
		expect(pngScale({ width: MAX_SIDE / 2, height: 10 })).toBe(2)
	})

	it('is lowered for a large drawing so that the longest side is at most MAX_SIDE', () => {
		const scale = pngScale({ width: 5000, height: 1000 })
		expect(scale).toBeCloseTo(MAX_SIDE / 5000, 10)
		expect(5000 * scale).toBeLessThanOrEqual(MAX_SIDE + 1e-6)
	})

	it('is lowered for a very large drawing, and the height decides when it is the longest side', () => {
		expect(pngScale({ width: 20000, height: 100 })).toBeCloseTo(0.4096, 10)
		expect(pngScale({ width: 100, height: 20000 })).toBeCloseTo(0.4096, 10)
	})

	it('honours the target and the maximum given', () => {
		expect(pngScale({ width: 10, height: 10 }, 3)).toBe(3)
		expect(pngScale({ width: 1000, height: 10 }, 2, 500)).toBeCloseTo(0.5, 10)
	})
})

describe('canvasSize', () => {
	it('multiplies the drawing by the scale, in whole pixels', () => {
		expect(canvasSize({ width: 100, height: 50 }, 2)).toEqual({ width: 200, height: 100 })
	})

	it('rounds each side to the nearest pixel', () => {
		expect(canvasSize({ width: 10.4, height: 10.6 }, 1)).toEqual({ width: 10, height: 11 })
	})

	it('is at least one pixel on each side', () => {
		expect(canvasSize({ width: 0.1, height: 0.2 }, 1)).toEqual({ width: 1, height: 1 })
	})

	it('never goes past MAX_SIDE, nor past the maximum given', () => {
		expect(canvasSize({ width: 20000, height: 100 }, 2)).toEqual({ width: MAX_SIDE, height: 200 })
		expect(canvasSize({ width: 300, height: 100 }, 2, 250)).toEqual({ width: 250, height: 200 })
	})

	it('keeps a canvas within MAX_SIDE for the scale pngScale chooses', () => {
		for (const size of [
			{ width: 1, height: 1 },
			{ width: 5000, height: 1000 },
			{ width: 1_000_000, height: 1 },
			{ width: 1, height: 1_000_000 }
		]) {
			const dims = canvasSize(size, pngScale(size))
			expect(dims.width).toBeLessThanOrEqual(MAX_SIDE)
			expect(dims.height).toBeLessThanOrEqual(MAX_SIDE)
			expect(Number.isInteger(dims.width) && Number.isInteger(dims.height)).toBe(true)
		}
	})
})

describe('standaloneSvg', () => {
	it('gives the drawing explicit width and height, and keeps its own viewBox', () => {
		const out = standaloneSvg('<svg viewBox="0 0 10 10"><rect width="10" height="10"/></svg>', { width: 300, height: 200 })
		expect(out).not.toBeNull()
		expect(out).toMatch(/<svg[^>]*\bwidth="300"/)
		expect(out).toMatch(/<svg[^>]*\bheight="200"/)
		expect(out).toMatch(/<svg[^>]*\bviewBox="0 0 10 10"/)
	})

	it('adds a viewBox from the size when the drawing has none', () => {
		const out = standaloneSvg('<svg><circle r="4"/></svg>', { width: 120, height: 40 })
		expect(out).toMatch(/<svg[^>]*\bviewBox="0 0 120 40"/)
	})

	it('replaces relative sizes such as width="50%" by the explicit ones', () => {
		const out = standaloneSvg('<svg width="50%" height="100%" viewBox="0 0 4 4"></svg>', { width: 64, height: 32 })
		expect(out).toMatch(/<svg[^>]*\bwidth="64"/)
		expect(out).toMatch(/<svg[^>]*\bheight="32"/)
		expect(out).not.toContain('50%')
	})

	it('declares the SVG namespace', () => {
		const out = standaloneSvg('<svg viewBox="0 0 4 4"></svg>', { width: 4, height: 4 })
		expect(out).toContain('xmlns="http://www.w3.org/2000/svg"')
	})

	it('removes the style attribute of the root element', () => {
		const out = standaloneSvg('<svg style="max-width:100%;background:red" viewBox="0 0 4 4"></svg>', { width: 4, height: 4 })
		expect(out).not.toMatch(/<svg[^>]*\sstyle=/)
		expect(out).not.toContain('max-width')
	})

	it('inserts the font css as the first child, in a style element', () => {
		const css = "@font-face{font-family:\"Inter\";src:url(data:font/woff2;base64,AAAA)}"
		const out = standaloneSvg('<svg viewBox="0 0 4 4"><rect width="4" height="4"/></svg>', { width: 4, height: 4 }, css)
		expect(out).toBeTruthy()
		const styleAt = out!.indexOf('<style')
		const rectAt = out!.indexOf('<rect')
		expect(styleAt).toBeGreaterThan(-1)
		expect(styleAt).toBeLessThan(rectAt)
		expect(out).toContain(css)
	})

	it('adds no style element when there is no font css', () => {
		const out = standaloneSvg('<svg viewBox="0 0 4 4"><rect width="4" height="4"/></svg>', { width: 4, height: 4 })
		expect(out).not.toContain('<style')
	})

	it('returns null when there is no svg root', () => {
		expect(standaloneSvg('<div><svg/></div>', { width: 4, height: 4 })).toBeNull()
		expect(standaloneSvg('<html><body></body></html>', { width: 4, height: 4 })).toBeNull()
		expect(standaloneSvg('', { width: 4, height: 4 })).toBeNull()
	})
})

describe('a drawing serialised as HTML', () => {
	// svg-viewer.ts builds `frame.svg` with `root.outerHTML` (an HTML document). HTML writes a
	// non-breaking space as `&nbsp;`, which an XML document does not define: standaloneSvg reads
	// such a drawing as HTML and serialises it back as XML.
	it('keeps a drawing whose text holds a non-breaking space, as it is serialised by svg-viewer.ts', () => {
		const doc = new DOMParser().parseFromString('<svg viewBox="0 0 40 10"><text x="2" y="8">a\u00a0b</text></svg>', 'text/html')
		const frameSvg = doc.querySelector('svg')?.outerHTML ?? ''
		expect(standaloneSvg(frameSvg, { width: 40, height: 10 })).not.toBeNull()
	})
})

describe('fontFamilies', () => {
	it('lists the families a drawing names, lower case, without the generic ones', () => {
		const svg = '<svg><text font-family="Inter, sans-serif">a</text><text style="font-family: \'Roboto Mono\', monospace">b</text></svg>'
		expect(fontFamilies(svg)).toEqual(['inter', 'roboto mono'])
	})

	it('reads quoted names, in attributes and in styles', () => {
		expect(fontFamilies('<text font-family="&quot;Fira Code&quot;, monospace">x</text>')).toEqual(['fira code'])
		expect(fontFamilies("<style>text{font-family:'Source Serif Pro',serif}</style>")).toEqual(['source serif pro'])
	})

	it('lists each family once, whatever its case', () => {
		expect(fontFamilies('<text font-family="Inter">a</text><text font-family="inter, Inter">b</text>')).toEqual(['inter'])
	})

	it('is empty when only generic families or none are named', () => {
		expect(fontFamilies('<text font-family="serif">a</text>')).toEqual([])
		expect(fontFamilies('<text font-family="system-ui, ui-monospace">a</text>')).toEqual([])
		expect(fontFamilies('<svg><rect/></svg>')).toEqual([])
	})
})

describe('effectiveBackground', () => {
	afterEach(() => {
		document.body.innerHTML = ''
	})

	it('gives the fallback without an element', () => {
		expect(effectiveBackground(null, '#ffffff')).toBe('#ffffff')
	})

	it('takes the first opaque background from the element up', () => {
		document.body.innerHTML = '<div id="outer" style="background-color: rgb(10, 20, 30)"><figure id="inner" style="background-color: transparent"></figure></div>'
		const inner = document.getElementById('inner')
		expect(effectiveBackground(inner, '#ffffff')).toBe('rgb(10, 20, 30)')
	})

	it('takes the element itself when it is opaque', () => {
		document.body.innerHTML = '<div id="own" style="background-color: rgb(1, 2, 3)"></div>'
		expect(effectiveBackground(document.getElementById('own'), '#ffffff')).toBe('rgb(1, 2, 3)')
	})

	it('falls back when every ancestor is transparent', () => {
		document.body.innerHTML = '<div id="clear" style="background-color: rgba(0, 0, 0, 0)"></div>'
		expect(effectiveBackground(document.getElementById('clear'), '#111827')).toBe('#111827')
	})
})

describe('svgToPng', () => {
	const SVG = '<svg viewBox="0 0 120 40"><rect width="120" height="40" fill="red"/></svg>'

	type Fake = {
		ctx: { fillStyle: string; calls: string[]; fillRect: ReturnType<typeof vi.fn>; drawImage: ReturnType<typeof vi.fn> }
		canvas: { width: number; height: number; getContext: ReturnType<typeof vi.fn>; toBlob: ReturnType<typeof vi.fn> }
	}

	let fake: Fake
	let revoked: string[]
	let created: string[]
	let decodeError: Error | null
	let originalCreate: typeof URL.createObjectURL | undefined
	let originalRevoke: typeof URL.revokeObjectURL | undefined
	let originalCreateElement: typeof document.createElement
	let imageSrc: string | null

	function makeFake(overrides: Partial<{ context: unknown; blob: Blob | null; clampWidth: number }> = {}): Fake {
		const calls: string[] = []
		const ctx = {
			fillStyle: '',
			calls,
			fillRect: vi.fn(() => calls.push(`fillRect:${ctx.fillStyle}`)),
			drawImage: vi.fn(() => calls.push('drawImage'))
		}
		const canvas = {
			_w: 0,
			_h: 0,
			get width() {
				return this._w
			},
			set width(value: number) {
				this._w = overrides.clampWidth !== undefined ? Math.min(value, overrides.clampWidth) : value
			},
			get height() {
				return this._h
			},
			set height(value: number) {
				this._h = value
			},
			getContext: vi.fn(() => ('context' in overrides ? overrides.context : ctx)),
			toBlob: vi.fn((callback: (blob: Blob | null) => void) => {
				callback('blob' in overrides ? (overrides.blob ?? null) : new Blob([new Uint8Array([137, 80, 78, 71])], { type: 'image/png' }))
			})
		}
		return { ctx, canvas: canvas as unknown as Fake['canvas'] }
	}

	class FakeImage {
		decoding = ''
		set src(value: string) {
			imageSrc = value
		}
		get src() {
			return imageSrc ?? ''
		}
		decode() {
			return decodeError ? Promise.reject(decodeError) : Promise.resolve()
		}
	}

	afterEach(() => {
		URL.createObjectURL = originalCreate as typeof URL.createObjectURL
		URL.revokeObjectURL = originalRevoke as typeof URL.revokeObjectURL
		document.createElement = originalCreateElement
		vi.unstubAllGlobals()
	})

	function install(overrides: Parameters<typeof makeFake>[0] = {}): Fake {
		fake = makeFake(overrides)
		revoked = []
		created = []
		decodeError = null
		imageSrc = null
		originalCreate = URL.createObjectURL
		originalRevoke = URL.revokeObjectURL
		originalCreateElement = document.createElement.bind(document)
		let counter = 0
		URL.createObjectURL = vi.fn((blob: Blob) => {
			created.push(`blob:${++counter}`)
			void blob
			return created[created.length - 1] as string
		}) as typeof URL.createObjectURL
		URL.revokeObjectURL = vi.fn((url: string) => {
			revoked.push(url)
		}) as typeof URL.revokeObjectURL
		document.createElement = ((tag: string, options?: ElementCreationOptions) =>
			tag.toLowerCase() === 'canvas' ? fake.canvas : originalCreateElement(tag, options)) as typeof document.createElement
		vi.stubGlobal('Image', FakeImage)
		return fake
	}

	it('draws the whole drawing at the scale, on the background, then encodes a PNG', async () => {
		install()
		const blob = await svgToPng(SVG, { width: 120, height: 40 }, '#112233')
		expect(blob.type).toBe('image/png')
		expect(fake.canvas.width).toBe(240)
		expect(fake.canvas.height).toBe(80)
		expect(fake.ctx.fillRect).toHaveBeenCalledWith(0, 0, 240, 80)
		expect(fake.ctx.drawImage).toHaveBeenCalledWith(expect.anything(), 0, 0, 240, 80)
	})

	it('fills the background before the image is drawn on it', async () => {
		install()
		await svgToPng(SVG, { width: 120, height: 40 }, '#112233')
		expect(fake.ctx.calls).toEqual(['fillRect:#112233', 'drawImage'])
	})

	it('draws at the fallback size when the drawing has no size', async () => {
		install()
		await svgToPng(SVG, null, '#ffffff')
		expect(fake.canvas.width).toBe(FALLBACK_SIZE.width * TARGET_SCALE)
		expect(fake.canvas.height).toBe(FALLBACK_SIZE.height * TARGET_SCALE)
	})

	it('decodes the drawing from a temporary URL and revokes that URL on success', async () => {
		install()
		await svgToPng(SVG, { width: 120, height: 40 }, '#ffffff')
		expect(created).toHaveLength(1)
		expect(imageSrc).toBe(created[0])
		expect(revoked).toEqual(created)
	})

	it('revokes the URL and rejects when the image cannot be decoded', async () => {
		install()
		decodeError = new Error('decode failed')
		await expect(svgToPng(SVG, { width: 120, height: 40 }, '#ffffff')).rejects.toThrow('decode failed')
		expect(revoked).toEqual(created)
		expect(created).toHaveLength(1)
	})

	it('revokes the URL and throws a PngError when no 2D context is available', async () => {
		install({ context: null })
		await expect(svgToPng(SVG, { width: 120, height: 40 }, '#ffffff')).rejects.toBeInstanceOf(PngError)
		expect(revoked).toEqual(created)
	})

	it('throws a PngError when the canvas keeps less than asked (too large to draw)', async () => {
		install({ clampWidth: 100 })
		await expect(svgToPng(SVG, { width: 120, height: 40 }, '#ffffff')).rejects.toBeInstanceOf(PngError)
		expect(revoked).toEqual(created)
	})

	it('revokes the URL and throws a PngError when the canvas gives no PNG', async () => {
		install({ blob: null })
		await expect(svgToPng(SVG, { width: 120, height: 40 }, '#ffffff')).rejects.toBeInstanceOf(PngError)
		expect(revoked).toEqual(created)
	})

	it('throws a PngError, without any URL, when the text is not an SVG document', async () => {
		install()
		await expect(svgToPng('<div>pas un dessin</div>', { width: 120, height: 40 }, '#ffffff')).rejects.toBeInstanceOf(PngError)
		expect(created).toEqual([])
		expect(revoked).toEqual([])
	})
})
