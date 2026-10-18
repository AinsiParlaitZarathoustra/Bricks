// Run through playwright-cli (a browser session on the local Vite server):
// playwright-cli run-code --filename=tests/harness/finalization.browser.js
// Only the test harness is controlled. Native IPC and clipboard are mocked explicitly.
async (page) => {
  const checks = []
  const ensure = (value, message) => { if (!value) throw new Error(message); checks.push(message) }
  const replace = (text, streaming = false) => page.evaluate(({ text, streaming }) => window.__markdownControl.replace(text, streaming), { text, streaming })
  await page.goto('http://127.0.0.1:4175/tests/harness/markdown.html?fixture=simple&fast=1')
  await page.getByRole('button', { name: 'Lancer', exact: true }).click()
  await page.waitForFunction(() => window.__harness?.fixture === 'simple', null, { timeout: 30000, polling: 50 })
  const simple = await page.evaluate(() => window.__harness)
  ensure(!simple.katexLoaded && !simple.mermaidLoaded, 'Ordinary Markdown loads neither KaTeX nor Mermaid')

  const browserMath = await page.evaluate(async () => {
    const { MarkdownDocument, renderReference } = await import('/src/lib/features/conversation/markdown/document.ts')
    const { loadKatex } = await import('/src/lib/features/conversation/markdown/math.ts')
    const math = await loadKatex()
    const doc = new MarkdownDocument({ idPrefix: 'browser' })
    doc.setMathRenderer(math)
    let source = '$\\gdef\\qq{AA}$\n\n$\\qq$\n\nTexte.\n\nFin.'
    const first = doc.update(source, false)
    source += ' Suite.'
    const next = doc.update(source, false)
    const reused = doc.stats.mathRenders === 0 && next[0] === first[0] && next[1] === first[1]
    const finalEqual = JSON.stringify(doc.update(source, true)) === JSON.stringify(renderReference(source, { idPrefix: 'browser', math }))
    const replaced = doc.update('$\\qq$', true).some(b => b.kind === 'html' && b.html.includes('class="math-error"'))
    return { reused, finalEqual, replaced }
  })
  ensure(browserMath.reused && browserMath.finalEqual && browserMath.replaced, 'Real KaTeX: ordered macros, frozen formulas reused, replaced source clears macros')

  await replace('# Premier\n\n## Deuxième\n\n### Troisième\n\nTexte[^n].\n\n[^n]: Une formule $x^2$.')
  await page.waitForFunction(() => !!document.querySelector('.md-footnotes .katex'), null, { timeout: 30000, polling: 50 })
  await page.getByRole('button', { name: 'Sommaire', exact: true }).click()
  await page.getByRole('button', { name: 'Troisième', exact: true }).click()
  ensure(await page.evaluate(() => document.activeElement?.textContent === 'Troisième'), 'Table of contents scrolls and focuses its heading')
  await page.locator('a[data-fn]').click()
  ensure(await page.evaluate(() => document.activeElement?.id.includes('-fn:n')), 'Footnote reference focuses its definition')
  await page.locator('a[data-fn-back]').click()
  ensure(await page.evaluate(() => document.activeElement?.id.includes('-fnref:n-1')), 'Footnote return focuses its reference')

  await replace('```ts\nconst first = 1;', true)
  await replace('```ts\nconst first = 1;\nconst last = "received";', true)
  await page.evaluate(() => window.__markdownControl.stop())
  ensure((await page.locator('.md').innerText()).includes('const last = "received";'), 'Stopping between throttled updates retains the last received partial fence')

  await replace('```diff\n--- a/a.ts\n+++ b/a.ts\n@@ -10,2 +40,2 @@\n same\n-old\n+new\n```')
  ensure(await page.locator('.diff-block [role="region"] .grid').evaluateAll(rows => {
    const context = rows.find(row => row.lastElementChild?.textContent === ' same')
    const added = rows.find(row => row.lastElementChild?.textContent === '+new')
    return context?.children[0]?.textContent === '10' && context?.children[1]?.textContent === '40' && added?.children[1]?.textContent === '41'
  }), 'Proposed diff displays the old and new hunk start numbers correctly')

  const diagram = 'flowchart TD\nA-->B'
  await replace('```mermaid\n' + diagram + '\n```')
  await page.locator('.mermaid-block svg').waitFor({ timeout: 30000 })
  await page.getByRole('button', { name: 'Zoom avant', exact: true }).click()
  ensure(await page.getByRole('button', { name: '125 %', exact: true }).isVisible(), 'Mermaid zoom reaches 125 percent')
  await page.getByRole('button', { name: 'Agrandir', exact: true }).click()
  await page.getByRole('dialog').waitFor()
  ensure(await page.evaluate(() => {
    const ids = [...document.querySelectorAll('.mermaid-svg [id]')].map(n => n.id)
    return ids.length > 0 && new Set(ids).size === ids.length
  }), 'Enlarged diagram has no duplicated SVG ids')
  await page.keyboard.press('Escape')
  ensure(await page.getByRole('button', { name: 'Agrandir', exact: true }).evaluate(el => el === document.activeElement), 'Escape closes enlarged view and restores focus')
  await page.evaluate(() => {
    window.__savedSvg = null
    window.__TAURI_INTERNALS__ = { invoke: async (command, args) => {
      if (command !== 'save_svg_file') throw new Error('Unexpected IPC: ' + command)
      window.__savedSvg = args
      return null // native save cancellation is simulated, no disk write
    } }
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async text => { window.__copiedSource = text } } })
  })
  await page.getByRole('button', { name: 'Copier la source', exact: true }).click()
  ensure(await page.evaluate(source => window.__copiedSource === source, diagram), 'Copy passes the exact Mermaid source to the clipboard adapter (mock)')
  await page.getByRole('button', { name: 'Exporter en SVG', exact: true }).click()
  ensure(await page.evaluate(() => {
    const text = window.__savedSvg?.contents
    if (!text) return false
    const parsed = new DOMParser().parseFromString(text, 'image/svg+xml')
    return !parsed.querySelector('parsererror, script, foreignObject') && parsed.documentElement.localName === 'svg' && text.includes('xmlns="http://www.w3.org/2000/svg"')
  }), 'Export passes a standalone valid SVG to native IPC (mock, no file written)')
  await replace('```mermaid\nflowchart TD\nX-->Y', true)
  // The existing 80 ms streaming cadence is preserved; wait for its pending tick.
  await page.waitForFunction(() => !document.querySelector('.mermaid-block svg') && !document.querySelector('button[aria-label="Exporter en SVG"]'), null, { timeout: 30000, polling: 50 })
  ensure(await page.locator('.mermaid-block svg').count() === 0 && await page.getByRole('button', { name: 'Exporter en SVG', exact: true }).count() === 0, 'Replacing a diagram by an open fence removes its previous SVG and export action')

  const externalRequests = []
  const track = request => { if (!new URL(request.url()).hostname.match(/^(127\.0\.0\.1|localhost)$/)) externalRequests.push(request.url()) }
  page.on('request', track)
  await page.goto('http://127.0.0.1:4175/tests/harness/markdown.html?fixture=hostile&fast=1')
  await page.getByRole('button', { name: 'Lancer', exact: true }).click()
  await page.waitForFunction(() => window.__harness?.fixture === 'hostile', null, { timeout: 30000, polling: 50 })
  ensure(await page.evaluate(() => {
    const root = document.querySelector('.md')
    return !root.querySelector('script, iframe, object, embed, img, foreignObject') &&
      [...root.querySelectorAll('*')].every(el => el.getAttributeNames().every(name => !/^on/i.test(name))) &&
      [...root.querySelectorAll('[href]')].every(el => /^(?:https?:|mailto:|#)/i.test(el.getAttribute('href')))
  }), 'Hostile fixture leaves no active tag, event handler or dangerous URL in real DOM')
  ensure(externalRequests.length === 0, 'Hostile fixture triggers no external network request')
  page.off('request', track)
  return { browser: 'Chromium (real DOM)', checks, simple, limits: ['Native IPC and clipboard mocked; no native export or native cancellation claimed'] }
}
