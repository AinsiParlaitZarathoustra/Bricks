#!/usr/bin/env node
import { createServer } from 'node:http'
import { readFileSync, readdirSync, writeFileSync, mkdirSync, rmSync, existsSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { spawn } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import { randomUUID } from 'node:crypto'

const appRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const repoRoot = path.resolve(appRoot, '../..')
const playwrightCliVersion = '0.1.22'
const tokenClasses = [
	'hljs-keyword',
	'hljs-string',
	'hljs-addition',
	'hljs-deletion',
	'hljs-number',
	'hljs-template-variable',
	'hljs-comment',
	'hljs-title class_',
]
function usage() {
	return `Usage: node scripts/compare-css-centralization.mjs --before-root DIR [options]

Options:
	--before-root DIR       Baseline checkout containing src/lib and dist (required)
	--after-root DIR        Current checkout containing src/lib and dist (default: ${appRoot})
	--output FILE           Full JSON report (default: ${path.join(repoRoot, 'output/playwright/css-centralization-comparison.json')})
	--allow-property NAME   Treat this computed property as an expected difference; repeatable
	--help                  Show this help

Both roots must already contain a production build in dist/. Install the
Playwright Chromium browser once with:
  npx --yes --package=@playwright/cli@${playwrightCliVersion} playwright-cli install-browser chromium
The comparison uses Playwright CLI;
set PLAYWRIGHT_CLI to an installed playwright-cli executable to avoid the
transient npx download.`
}

function parseArgs(args) {
	const options = { afterRoot: appRoot, allowProperties: [] }
	for (let i = 0; i < args.length; i += 1) {
		const arg = args[i]
		if (arg === '--help' || arg === '-h') {
			console.log(usage())
			process.exit(0)
		}
		if (arg === '--before-root' || arg === '--after-root' || arg === '--output' || arg === '--allow-property') {
			const value = args[++i]
			if (!value || value.startsWith('--')) throw new Error(`Missing value for ${arg}`)
			if (arg === '--before-root') options.beforeRoot = path.resolve(value)
			if (arg === '--after-root') options.afterRoot = path.resolve(value)
			if (arg === '--output') options.output = path.resolve(value)
			if (arg === '--allow-property') options.allowProperties.push(value)
			continue
		}
		throw new Error(`Unknown argument: ${arg}`)
	}
	if (!options.beforeRoot) throw new Error('--before-root is required')
	options.output ??= path.join(repoRoot, 'output/playwright/css-centralization-comparison.json')
	return options
}

function walkSvelteFiles(dir) {
	if (!existsSync(dir)) return []
	const files = []
	for (const entry of readdirSync(dir, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
		const fullPath = path.join(dir, entry.name)
		if (entry.isDirectory()) files.push(...walkSvelteFiles(fullPath))
		else if (entry.isFile() && entry.name.endsWith('.svelte')) files.push(fullPath)
	}
	return files
}

function collectClassCases(beforeRoot) {
	const libRoot = path.join(beforeRoot, 'src/lib')
	const cases = []
	for (const file of walkSvelteFiles(libRoot)) {
		if (file.includes(`${path.sep}components${path.sep}ui${path.sep}`)) continue
		const source = readFileSync(file, 'utf8')
		for (const [index, match] of [...source.matchAll(/\bclass="([^"{}]+)"/g)].entries()) {
			const classes = match[1]
			if (classes.includes('text-[') || /\btext-(ui|caption|compact|code)\b/.test(classes) || classes.includes('focus-visible:') || classes.includes('bg-')) {
				cases.push({ id: `${path.relative(path.join(beforeRoot, 'src'), file).split(path.sep).join('/')}#${index}`, classes })
			}
		}
	}
	return cases.concat([
		{ id: 'size13-inherited', classes: 'text-[13px] text-muted-foreground leading-relaxed' },
		{ id: 'size125-inherited', classes: 'text-[12.5px] font-mono leading-5' },
		{ id: 'size11-inherited', classes: 'text-[11px] text-muted-foreground' },
		{ id: 'file-icon', classes: 'file-icon' },
	])
}

function scopeClass(css, selector) {
	return css.match(selector)?.[1] ?? ''
}

function htmlForRoot(root, cases) {
	const dist = path.join(root, 'dist')
	const assetDir = path.join(dist, 'assets')
	if (!existsSync(path.join(root, 'src/lib')) || !existsSync(assetDir)) {
		throw new Error(`${root} must contain src/lib and a built dist/assets directory. Build both revisions before comparing.`)
	}
	const cssFiles = readdirSync(assetDir).filter((name) => name.endsWith('.css')).sort()
	if (cssFiles.length === 0) throw new Error(`No CSS assets found in ${assetDir}`)
	const css = cssFiles.map((name) => readFileSync(path.join(assetDir, name), 'utf8')).join('\n')
	const markdownScope = scopeClass(css, /\.md\.(svelte-[\w-]+)/)
	const readerScope = scopeClass(css, /\.hljs\.(svelte-[\w-]+)/)
	const hasMarkdownMarker = /\.syntax-markdown\.md\b/.test(css)
	const hasReaderMarker = /\.hljs\.syntax-reader\b/.test(css)
	const mapSize = (classes, token, legacy) => css.includes(`.${token}`) ? classes.replaceAll(legacy, token) : classes
	const mappedCases = cases.map(({ id, classes }) => ({
		id,
		classes: mapSize(mapSize(mapSize(classes, 'text-ui', 'text-[13px]'), 'text-compact', 'text-[12.5px]'), 'text-caption', 'text-[11px]'),
	}))
	const roots = {
		md: ['md', 'font-mono', hasMarkdownMarker ? 'syntax-markdown' : '', markdownScope].filter(Boolean).join(' '),
		reader: ['hljs', 'font-mono', hasReaderMarker ? 'syntax-reader' : '', readerScope].filter(Boolean).join(' '),
	}
	const config = JSON.stringify({ cases: mappedCases, roots, tokens: tokenClasses }).replaceAll('<', '\\u003c')
	const stylesheetLinks = cssFiles.map((file) => `<link rel="stylesheet" href="/assets/${encodeURIComponent(file)}">`).join('')
	const html = `<!doctype html><html><head><meta charset="utf-8">${stylesheetLinks}</head><body><main id="samples"></main><script>
window.fixture = ${config};
const main = document.querySelector('#samples');
for (const item of fixture.cases) {
  const el = document.createElement('button');
  el.dataset.case = item.id;
  el.className = item.classes;
  el.textContent = 'Aa Bricks 0123';
  main.append(el);
}
for (const mode of ['md', 'reader']) {
  const wrap = document.createElement('div');
  wrap.className = fixture.roots[mode];
  for (const token of fixture.tokens) {
    const el = document.createElement('span');
    el.className = token;
    el.dataset.case = mode + ':' + token;
    el.textContent = 'Bricks';
    wrap.append(el);
  }
  main.append(wrap);
}
</script></body></html>`
	return { dist, cssFiles, cases: mappedCases, html }
}

function contentType(file) {
	return ({
		'.css': 'text/css; charset=utf-8',
		'.html': 'text/html; charset=utf-8',
		'.js': 'text/javascript; charset=utf-8',
		'.json': 'application/json; charset=utf-8',
		'.png': 'image/png',
		'.svg': 'image/svg+xml',
		'.ttf': 'font/ttf',
		'.woff': 'font/woff',
		'.woff2': 'font/woff2',
	})[path.extname(file).toLowerCase()] ?? 'application/octet-stream'
}

async function serve(dist, fixtureHtml) {
	const server = createServer((request, response) => {
		const pathname = new URL(request.url ?? '/', 'http://localhost').pathname
		if (pathname === '/css-compare.html') {
			response.writeHead(200, { 'content-type': 'text/html; charset=utf-8', 'cache-control': 'no-store' }).end(fixtureHtml)
			return
		}
		const file = path.resolve(dist, `.${decodeURIComponent(pathname)}`)
		if (file !== dist && !file.startsWith(`${dist}${path.sep}`)) {
			response.writeHead(403).end('Forbidden')
			return
		}
		try {
			const body = readFileSync(file)
			response.writeHead(200, { 'content-type': contentType(file), 'cache-control': 'no-store' }).end(body)
		} catch {
			response.writeHead(404).end('Not found')
		}
	})
	await new Promise((resolve, reject) => {
		server.once('error', reject)
		server.listen(0, '127.0.0.1', resolve)
	})
	const address = server.address()
	return { server, url: `http://127.0.0.1:${address.port}/css-compare.html` }
}

function browserCode(config) {
	return `async (page) => {
	const config = ${JSON.stringify(config)};
	const states = ['light', 'dark', 'custom'];
	const results = {};
	const loadedFontFaces = {};
	const pages = { before: page, after: await page.context().newPage() };
	for (const kind of ['before', 'after']) {
		const target = pages[kind];
		await target.goto(config.urls[kind]);
		await target.evaluate(() => document.fonts.ready);
		loadedFontFaces[kind] = await target.evaluate(() => Array.from(document.fonts)
			.filter((face) => face.status === 'loaded')
			.map((face) => ({ family: face.family, style: face.style, weight: face.weight, status: face.status }))
			.sort((a, b) => a.family.localeCompare(b.family) || a.style.localeCompare(b.style) || a.weight.localeCompare(b.weight)));
		results[kind] = {};
		for (const state of states) {
			results[kind][state] = await target.evaluate((state) => {
				document.documentElement.className = state === 'dark' ? 'dark' : '';
				document.documentElement.removeAttribute('style');
				if (state === 'custom') for (const [key, value] of Object.entries({ '--primary': '#a4512c', '--background': '#fbfaf7', '--foreground': '#1d1b18', '--code-bg': '#eae5db', '--muted-foreground': '#64615b' })) document.documentElement.style.setProperty(key, value);
				for (const animation of document.getAnimations()) { animation.pause(); animation.currentTime = 0; }
				const out = {};
				for (const el of document.querySelectorAll('[data-case]')) {
					const style = getComputedStyle(el);
					const values = {};
					for (let i = 0; i < style.length; i += 1) {
						const property = style.item(i);
						if (!property.startsWith('--')) values[property] = style.getPropertyValue(property);
					}
					out[el.dataset.case] = values;
				}
				return out;
			}, state);
		}
	}
	const differences = [];
	for (const state of states) {
		for (const [id, before] of Object.entries(results.before[state])) {
			const after = results.after[state][id];
			if (!after) {
				differences.push({ state, id, property: '<case>', before: 'present', after: 'missing' });
				continue;
			}
			for (const property of new Set([...Object.keys(before), ...Object.keys(after)])) {
				if (before[property] !== after[property]) differences.push({ state, id, property, before: before[property] ?? null, after: after[property] ?? null });
			}
		}
	}
	return JSON.stringify({ states, caseCount: Object.keys(results.before.light).length, differences, loadedFontFaces, results });
}`
}

function runCli(args, session) {
	return new Promise((resolve, reject) => {
		const cli = process.env.PLAYWRIGHT_CLI
		const command = cli || 'npx'
		const prefix = cli ? [] : ['--yes', `--package=@playwright/cli@${playwrightCliVersion}`, 'playwright-cli']
		const child = spawn(command, [...prefix, `--session=${session}`, ...args], { cwd: path.join(repoRoot, 'output/playwright'), env: process.env, stdio: ['ignore', 'pipe', 'pipe'] })
		let stdout = ''
		let stderr = ''
		child.stdout.setEncoding('utf8').on('data', (chunk) => { stdout += chunk })
		child.stderr.setEncoding('utf8').on('data', (chunk) => { stderr += chunk })
		child.once('error', reject)
		child.once('close', (code) => code === 0 ? resolve(stdout.trim()) : reject(new Error(`${command} exited ${code}:\n${stderr || stdout}`)))
	})
}

async function main() {
	const options = parseArgs(process.argv.slice(2))
	const cases = collectClassCases(options.beforeRoot)
	mkdirSync(path.join(repoRoot, 'output/playwright'), { recursive: true })
	const before = htmlForRoot(options.beforeRoot, cases)
	const after = htmlForRoot(options.afterRoot, cases)
	const [beforeServer, afterServer] = await Promise.all([serve(before.dist, before.html), serve(after.dist, after.html)])
	const browserFile = path.join(tmpdir(), `css-centralization-${randomUUID()}.js`)
	const session = `css-compare-${process.pid}`
	let opened = false
	try {
		const browserSource = browserCode({ urls: { before: beforeServer.url, after: afterServer.url } })
		writeFileSync(browserFile, browserSource)
		await runCli(['open', beforeServer.url, '--browser=chromium'], session)
		opened = true
		const raw = await runCli(['--raw', 'run-code', `--filename=${browserFile}`], session)
		let report = JSON.parse(raw)
		if (typeof report === 'string') report = JSON.parse(report)
		if (!Array.isArray(report.differences) || !report.loadedFontFaces || !report.results) {
			throw new Error(`Playwright CLI returned an unexpected result: ${raw.slice(0, 1200)}`)
		}
		const allow = new Set(options.allowProperties)
		const expected = report.differences.filter(({ property }) => allow.has(property))
		const unexpected = report.differences.filter(({ property }) => !allow.has(property))
		const output = {
			generatedAt: new Date().toISOString(),
			beforeRoot: options.beforeRoot,
			afterRoot: options.afterRoot,
			fixtures: {
				before: { cssAssets: before.cssFiles, cases: before.cases },
				after: { cssAssets: after.cssFiles, cases: after.cases },
			},
			states: report.states,
			caseCount: report.caseCount,
			expectedDifferenceProperties: [...allow].sort(),
			differenceCount: report.differences.length,
			expectedDifferenceCount: expected.length,
			unexpectedDifferenceCount: unexpected.length,
			differences: report.differences,
			expectedDifferences: expected,
			unexpectedDifferences: unexpected,
			loadedFontFaces: report.loadedFontFaces,
			computedStyles: report.results,
		}
		mkdirSync(path.dirname(options.output), { recursive: true })
		writeFileSync(options.output, `${JSON.stringify(output, null, 2)}\n`)
		console.log(`CSS comparison: ${report.caseCount} cases across ${report.states.join(', ')}.`)
		console.log(`Differences: ${expected.length} expected, ${unexpected.length} unexpected.`)
		console.log(`Full report: ${options.output}`)
		if (unexpected.length > 0) process.exitCode = 1
	} finally {
		if (opened) await runCli(['close'], session).catch(() => {})
		rmSync(browserFile, { force: true })
		await Promise.all([beforeServer.server, afterServer.server].map((server) => new Promise((resolve) => server.close(resolve))))
	}
}

try {
	await main()
} catch (error) {
	console.error(error instanceof Error ? error.message : error)
	process.exitCode = 1
}
