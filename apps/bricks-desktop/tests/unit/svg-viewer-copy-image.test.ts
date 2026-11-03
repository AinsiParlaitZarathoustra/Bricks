// @vitest-environment happy-dom
// SvgViewer.svelte: the image actions of a diagram. Most checks read its source: button
// order and labels, the clipboard written inside the click (no await before it), the
// button hidden without ClipboardItem, and the PNG export. The PNG size limit is run:
// the component is mounted with the IPC client and the rasteriser replaced by fakes, and
// a PNG over 32 Mio is refused without the backend being asked to save it.
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { flushSync, mount, unmount } from 'svelte'
import SvgViewer from '../../src/lib/features/conversation/markdown/SvgViewer.svelte'
import { bound, call } from './french-keys'

// The sanitizer is replaced by the identity: this file checks the export, not the cleaning
// (markdown-svg-vega-blocks and vega-chart cover it).
vi.mock('../../src/lib/features/conversation/markdown/sanitize', async (importOriginal) => ({
	...(await importOriginal<typeof import('../../src/lib/features/conversation/markdown/sanitize')>()),
	sanitizeSvg: (svg: string) => svg
}))

const ipc = vi.hoisted(() => ({ savePngFile: vi.fn(), saveSvgFile: vi.fn() }))
vi.mock('$lib/ipc/client', async (importOriginal) => ({
	...(await importOriginal<typeof import('../../src/lib/ipc/client')>()),
	savePngFile: ipc.savePngFile,
	saveSvgFile: ipc.saveSvgFile
}))

const rasteriser = vi.hoisted(() => ({ svgToPng: vi.fn() }))
vi.mock('../../src/lib/features/conversation/markdown/svg-png', async (importOriginal) => ({
	...(await importOriginal<typeof import('../../src/lib/features/conversation/markdown/svg-png')>()),
	svgToPng: rasteriser.svgToPng
}))

const SOURCE = readFileSync(
	resolve(process.cwd(), 'src/lib/features/conversation/markdown/SvgViewer.svelte'),
	'utf8'
)
const CURLY = '’'

/** The text of a function declared in the script, from its signature to the next top-level declaration. */
function functionBody(name: string): string {
	const start = SOURCE.indexOf(`function ${name}(`)
	expect(start, `function ${name} exists`).toBeGreaterThan(-1)
	const rest = SOURCE.slice(start + 1)
	const next = rest.search(/\n  (?:async )?function |\n  const action = /)
	return next < 0 ? SOURCE.slice(start) : SOURCE.slice(start, start + 1 + next)
}

describe('SvgViewer image actions: order and labels', () => {
	it('puts "Copier l’image" right after "Copier la source"', () => {
		const source = SOURCE.indexOf(bound('aria-label', 'Copier la source'))
		const image = SOURCE.indexOf(bound('aria-label', `Copier l${CURLY}image`))
		expect(source).toBeGreaterThan(-1)
		expect(image).toBeGreaterThan(source)
		const between = SOURCE.slice(source, image)
		// No other control lies between them: the only aria-label before the image button is the source one.
		expect(between.match(/aria-label=/g)?.length ?? 0).toBe(1)
		expect(between).toContain('{#if canCopyImage}')
	})

	it('gives the image button a title equal to its aria-label', () => {
		expect(SOURCE).toContain(`${bound('title', `Copier l${CURLY}image`)} ${bound('aria-label', `Copier l${CURLY}image`)}`)
	})

	it('puts the PNG export after the SVG export, both behind the download permission', () => {
		const svg = SOURCE.indexOf(bound('aria-label', 'Exporter en SVG'))
		const png = SOURCE.indexOf(bound('aria-label', 'Exporter en PNG'))
		expect(svg).toBeGreaterThan(-1)
		expect(png).toBeGreaterThan(svg)
		expect(SOURCE.slice(png - 200, png)).toMatch(/\{#if options\.allowDownload && frame\.svg\}/)
	})

	it('disables the image button when there is no drawing', () => {
		const button = SOURCE.slice(SOURCE.indexOf(bound('aria-label', `Copier l${CURLY}image`)) - 200, SOURCE.indexOf(bound('aria-label', `Copier l${CURLY}image`)) + 200)
		expect(button).toContain('disabled={!frame.svg}')
	})
})

describe('SvgViewer copyImage', () => {
	const body = functionBody('copyImage')

	it('writes the clipboard synchronously, before any await', () => {
		// Comments aside (the code's own explanation names the await), no await in the body.
		expect(body.replace(/\/\/.*$/gm, '')).not.toMatch(/\bawait\b/)
		const write = body.indexOf('navigator.clipboard.write(')
		expect(write).toBeGreaterThan(-1)
		// The image is only a promise handed to ClipboardItem: the write starts inside the click.
		expect(body.indexOf('const image = png()')).toBeLessThan(write)
		expect(body).toContain("new ClipboardItem({ 'image/png': image })")
	})

	it('is guarded by the copy permission, a drawing and the clipboard support', () => {
		expect(body).toMatch(/if \(!options\.allowCopy \|\| !frame\.svg \|\| !canCopyImage\) return/)
	})

	it('reports success only once the write and the image are both done', () => {
		expect(body).toContain('Promise.all([written, image])')
		expect(body).toContain(`flash(${call('Image copiée')})`)
		expect(body).toContain(`flash(${call('Copie impossible')})`)
	})

	it('reports a failure of the write synchronously, without leaving the image promise unhandled', () => {
		expect(body).toMatch(/catch \{\s*image\.catch\(\(\) => \{\}\)/)
	})
})

describe('SvgViewer clipboard support', () => {
	it('is detected from ClipboardItem and navigator.clipboard.write, and gates the button', () => {
		expect(SOURCE).toContain("typeof ClipboardItem !== 'undefined'")
		expect(SOURCE).toContain('navigator.clipboard?.write')
		const gate = SOURCE.indexOf('{#if canCopyImage}')
		const button = SOURCE.indexOf(bound('aria-label', `Copier l${CURLY}image`))
		const end = SOURCE.indexOf('{/if}', button)
		expect(gate).toBeGreaterThan(-1)
		expect(gate).toBeLessThan(button)
		expect(end).toBeGreaterThan(button)
	})
})

describe('SvgViewer PNG export', () => {
	it('names the file after the drawing, with a .png extension', () => {
		expect(functionBody('downloadPng')).toContain(".replace(/\\.svg$/i, '') + '.png'")
	})

	it('asks for the save only through savePngFile, and does nothing without a drawing', () => {
		const body = functionBody('downloadPng')
		expect(body).toMatch(/if \(!options\.allowDownload \|\| !frame\.svg\) return/)
		expect(body).toContain('savePngFile(name,')
	})
})

describe('SvgViewer PNG export, mounted: the size limit', () => {
	/** What `save_png_file` accepts at most (32 Mio), as SvgViewer.svelte states it. */
	const PNG_LIMIT = 32 * 1024 * 1024
	const DRAWING = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10" width="10" height="10"><rect width="10" height="10"/></svg>'
	let target: HTMLDivElement
	let app: ReturnType<typeof mount> | null = null

	function fakeBlob(size: number): Blob {
		return { size, type: 'image/png', arrayBuffer: async () => new ArrayBuffer(4) } as unknown as Blob
	}

	beforeEach(() => {
		vi.clearAllMocks()
		target = document.createElement('div')
		document.body.appendChild(target)
		app = mount(SvgViewer, { target, props: { svg: DRAWING, title: 'SVG', fileName: 'dessin.svg', variant: 'drawing' } })
		flushSync()
	})

	afterEach(() => {
		if (app) void unmount(app)
		app = null
		target.remove()
	})

	/** Clicks "Exporter en PNG" and lets the export settle; returns the message shown, if any. */
	async function exportPng(): Promise<string | null> {
		target.querySelector<HTMLButtonElement>('button[aria-label="Exporter en PNG"]')?.click()
		for (let i = 0; i < 5; i++) {
			await new Promise((resolve) => setTimeout(resolve, 0))
			flushSync()
		}
		return target.querySelector('[role="status"]')?.textContent ?? null
	}

	it('refuses a PNG of more than 32 Mio, and never asks the backend to save it', async () => {
		rasteriser.svgToPng.mockResolvedValue(fakeBlob(PNG_LIMIT + 1))
		expect(await exportPng()).toBe('PNG trop volumineux')
		expect(ipc.savePngFile).not.toHaveBeenCalled()
	})

	it('accepts a PNG of exactly 32 Mio and names it after the drawing', async () => {
		rasteriser.svgToPng.mockResolvedValue(fakeBlob(PNG_LIMIT))
		ipc.savePngFile.mockResolvedValue('/tmp/dessin.png')
		expect(await exportPng()).toBe('PNG enregistré')
		expect(ipc.savePngFile).toHaveBeenCalledWith('dessin.png', expect.any(Uint8Array))
	})
})
