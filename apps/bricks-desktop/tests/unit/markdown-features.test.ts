// @vitest-environment happy-dom
// Bricks features of assistant messages: alerts, footnotes, anchors, file references,
// clicks, Mermaid logic (bounds, ids, caches, streaming state), proposed diffs, and
// streams that end in the middle of a block. Simulated DOM (happy-dom): sanitizing is
// mocked here, as in the other Markdown tests, and covered by markdown-hostile.test.ts.
import { afterEach, describe, expect, it, vi } from 'vitest'
import { openUrl } from '@tauri-apps/plugin-opener'
import { markdownClicks } from '../../src/lib/features/conversation/markdown/clicks'
import { MarkdownDocument, renderReference } from '../../src/lib/features/conversation/markdown/document'
import { slugify } from '../../src/lib/features/conversation/markdown/extensions'
import { findFileReferences } from '../../src/lib/features/conversation/markdown/fileref-parse'
import type { RenderedBlock } from '../../src/lib/features/conversation/markdown/types'
import { MAX_SOURCE_LENGTH, createMermaidCache, renderMermaid } from '../../src/lib/features/conversation/markdown/mermaid'
import { patchFiles } from '../../src/lib/features/changes/patch'

vi.mock('../../src/lib/features/conversation/markdown/sanitize', () => ({
  sanitizeMarkdown: (html: string) => html,
  sanitizeSvg: (svg: string) => svg,
}))

vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn(async () => undefined) }))

const mermaidApi = vi.hoisted(() => ({
  initialize: vi.fn(),
  render: vi.fn(async (id: string) => ({ svg: `<svg id="${id}" viewBox="0 0 10 10"></svg>` })),
}))
vi.mock('mermaid', () => ({ default: mermaidApi }))

// ─── helpers ─────────────────────────────────────────────────────────────────

function htmlOf(blocks: RenderedBlock[]): string {
  return blocks.map((block) => (block.kind === 'html' ? block.html : '')).join('')
}

function render(source: string, idPrefix = 'x'): string {
  return htmlOf(renderReference(source, { idPrefix }))
}

function decode(text: string): string {
  return text
    .replace(/&lt;/g, '<')
    .replace(/&gt;/g, '>')
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&amp;/g, '&')
}

/** The visible text of rendered HTML (tags dropped, line breaks kept as spaces). */
function textOf(html: string): string {
  return decode(html.replace(/<br\s*\/?>/g, ' ').replace(/<[^>]+>/g, ''))
}

afterEach(() => {
  document.body.innerHTML = ''
  vi.clearAllMocks()
})

// ─── 1. Alerts ───────────────────────────────────────────────────────────────

describe('alerts', () => {
  it.each(['NOTE', 'tip', 'Important', 'WARNING', 'caution'])('%s is recognized whatever its letter case', (kind) => {
    const html = render(`> [!${kind}]\n> corps`)
    expect(html).toContain(`class="md-alert md-alert-${kind.toLowerCase()}"`)
    expect(html).toContain(`data-alert-icon="${kind.toLowerCase()}"`)
  })

  it('the marker must be alone on the first line of the quote', () => {
    for (const source of ['> [!NOTE] texte sur la ligne', '>[!TIP]x', '> [!INFO]\n> x', '> [!NOTES]\n> x', '> texte\n> [!NOTE]\n> suite', '>\n> [!NOTE]\n> x']) {
      expect(render(source), source).not.toContain('md-alert')
    }
  })

  it('a second marker inside an alert is text, not a second alert', () => {
    const html = render('> [!NOTE]\n> x\n> [!TIP]\n> y')
    expect(html.match(/class="md-alert md-alert-/g)).toHaveLength(1)
    expect(textOf(html)).toContain('[!TIP]')
  })

  it('is not recognized in a fenced block or in an indented block', () => {
    expect(render('```\n> [!WARNING]\n> x\n```')).not.toContain('md-alert')
    expect(render('    > [!TIP]\n    > x')).not.toContain('md-alert')
  })

  // The sprint explicitly requires nesting in real blockquotes.
  it('an alert inside a list item is recognized', () => {
    expect(render('- élément\n\n  > [!NOTE]\n  > x')).toContain('md-alert-note')
  })

  it('an alert inside a nested quote is recognized', () => {
    expect(render('> > [!NOTE]\n> > x')).toContain('md-alert-note')
  })
})

// ─── 2. Footnotes ────────────────────────────────────────────────────────────

describe('footnotes', () => {
  it('each reference of the same note has its own return target', () => {
    const html = render('A[^1] B[^1] C[^1]\n\n[^1]: note')
    const refIds = [...html.matchAll(/ id="(x-fnref:[^"]+)"/g)].map((match) => match[1])
    expect(refIds).toEqual(['x-fnref:1-1', 'x-fnref:1-2', 'x-fnref:1-3'])
    for (const id of refIds) expect(html).toContain(`href="#${id}"`)
  })

  it('a definition that arrives after its reference: literal text first, then a link', () => {
    const doc = new MarkdownDocument({ idPrefix: 's' })
    const first = htmlOf(doc.update('Texte réf[^1].', false))
    expect(first).toContain('Texte réf[^1].')
    expect(first).not.toContain('fn-ref')
    const second = htmlOf(doc.update('Texte réf[^1].\n\n[^1]: définition', false))
    expect(second).toContain('<sup class="fn-ref"><a href="#s-fn:1" id="s-fnref:1-1" data-fn>1</a></sup>')
    expect(second).toContain('<li id="s-fn:1">')
    expect(textOf(second)).toContain('définition')
  })

  it('an unresolved reference stays text, and an unused definition is not shown', () => {
    const html = render('A[^9] B\n\n[^1]: inutilisée')
    expect(html).toContain('A[^9] B')
    expect(html).not.toContain('fn-ref')
    expect(html).not.toContain('md-footnotes')
    expect(html).not.toContain('inutilisée')
  })

  it('two messages with the same note ids get ids of their own prefix', () => {
    const alpha = render('Texte[^1]\n\n[^1]: A', 'alpha')
    const beta = render('Texte[^1]\n\n[^1]: B', 'beta')
    expect(alpha).toContain('id="alpha-fn:1"')
    expect(beta).toContain('id="beta-fn:1"')
    expect(alpha).not.toContain('beta-')
    expect(beta).not.toContain('alpha-')
  })
})

// ─── 3. Anchors and headings ─────────────────────────────────────────────────

describe('headings and anchors', () => {
  it.each([
    ['日本語 タイトル', '日本語-タイトル'],
    ['Crème brûlée', 'creme-brulee'],
    ['Foo: bar (baz) & qux', 'foo-bar-baz--qux'],
    ['Résumé 😀 Test', 'resume--test'],
    ['!!!', 'section'],
  ])('the slug of %j is %j', (title, slug) => {
    expect(slugify(title)).toBe(slug)
  })

  it('Unicode headings get readable ids', () => {
    const html = render('## 日本語 タイトル\n\n## Crème brûlée\n')
    expect(html).toContain('<h3 id="x-日本語-タイトル">日本語 タイトル</h3>')
    expect(html).toContain('<h3 id="x-creme-brulee">Crème brûlée</h3>')
  })

  it('a setext title is a heading of its level, with its id', () => {
    const doc = new MarkdownDocument({ idPrefix: 'x' })
    const blocks = doc.update('Titre setext\n-------------\n\ntexte', true)
    expect(htmlOf(blocks)).toContain('<h3 id="x-titre-setext">Titre setext</h3>')
    expect(doc.headings).toEqual([{ id: 'x-titre-setext', text: 'Titre setext', level: 2 }])
  })

  it('a heading being streamed is listed, and the frozen headings keep their ids', () => {
    const doc = new MarkdownDocument({ idPrefix: 'h' })
    doc.update('# Premier\n\nTexte\n\n## Deuxième tit', false)
    expect(doc.headings.map((heading) => heading.id)).toEqual(['h-premier', 'h-deuxieme-tit'])
    doc.update('# Premier\n\nTexte\n\n## Deuxième titre\n\nfin', false)
    expect(doc.headings).toEqual([
      { id: 'h-premier', text: 'Premier', level: 1 },
      { id: 'h-deuxieme-titre', text: 'Deuxième titre', level: 2 },
    ])
  })

  it('a link to a heading must be written with its target in angle brackets when it has spaces', () => {
    // CommonMark: `[x](#Élève café)` is not a link; the target `<…>` form is, and so is the encoded form.
    expect(render('## Élève café\n\n[x](#Élève café)')).not.toContain('data-local-anchor')
    const html = render('## Élève café\n\n[x](<#Élève café>) et [y](#%C3%89l%C3%A8ve%20caf%C3%A9)')
    expect(html.match(/<a href="#x-eleve-cafe"[^>]*data-local-anchor>/g)).toHaveLength(2)
  })

  it('(défaut) a heading id is never the id of a footnote', () => {
    // `## Fn 1` takes the id `x-fn-1`, which is the id of the note [^1].
    const html = render('## Fn 1\n\nTexte[^1].\n\n[^1]: note')
    const ids = [...html.matchAll(/ id="([^"]+)"/g)].map((match) => match[1])
    expect(new Set(ids).size).toBe(ids.length)
  })
})

// ─── 4. File references and clicks ───────────────────────────────────────────

describe('file references in messages', () => {
  it('link targets: a line written with a colon, a range, encoded spaces', () => {
    const html = render('[x](src/a.ts:12-14) [y](src/a%20b.ts:3)')
    expect(html).toContain('<button type="button" class="file-ref" data-file-path="src/a.ts" data-line="12" data-end-line="14">x</button>')
    expect(html).toContain('<button type="button" class="file-ref" data-file-path="src/a b.ts" data-line="3">y</button>')
  })

  it('link targets that leave the project or use a scheme are not file buttons', () => {
    for (const target of ['../secret.ts#L1', '/etc/passwd.txt#L1', 'file:///etc/a.ts#L1']) {
      const html = render(`[x](${target})`)
      expect(html, target).not.toContain('file-ref')
      expect(html, target).not.toContain('href=')
    }
  })

  it('a URL with a path and a line stays a link', () => {
    const html = render('[x](https://example.com/src/a.rs:3)')
    expect(html).toContain('href="https://example.com/src/a.rs:3"')
    expect(html).not.toContain('file-ref')
  })

  it('a bare path followed by a full stop: the button has no full stop', () => {
    const html = render('Voir src/a.ts:3.')
    expect(html).toContain('<button type="button" class="file-ref" data-file-path="src/a.ts" data-line="3">src/a.ts:3</button>.')
  })

  it('bare paths: ranges count, out-of-project and absolute paths do not', () => {
    const html = render('Voir src/lib.rs:42-50, puis ../x.ts:1 et /abs/y.ts:2.')
    expect(html.match(/class="file-ref"/g)).toHaveLength(1)
    expect(html).toContain('data-line="42" data-end-line="50"')
  })

  it('inline code and fenced code are not read for file references', () => {
    const html = render('`src/a.ts:3`\n\n```\nsrc/b.ts:4\n```')
    expect(html).not.toContain('file-ref')
    expect(html).toContain('<code>src/a.ts:3</code>')
  })

  it('a line number glued to a word is not a line: no reference is read', () => {
    expect(findFileReferences('src/a.ts:3x')).toEqual([])
  })
})

describe('clicks on a rendered message', () => {
  /** A message mounted in a conversation container (`data-conversation-scroll`). */
  function mount(source: string, options?: Parameters<typeof markdownClicks>[1]) {
    const scroll = document.createElement('div')
    scroll.setAttribute('data-conversation-scroll', '')
    const message = document.createElement('div')
    message.innerHTML = render(source, 'x')
    scroll.append(message)
    document.body.append(scroll)
    const action = markdownClicks(message, options)
    return { scroll, message, action }
  }

  function click(element: Element): MouseEvent {
    const event = new MouseEvent('click', { bubbles: true, cancelable: true })
    element.dispatchEvent(event)
    return event
  }

  it('a file button reports exactly its reference, and never opens a link', () => {
    const onFileReference = vi.fn()
    const { message } = mount('Voir [a](src/a.ts#L3-L9) et NOTES.md, src/lib.rs:7.', { onFileReference })
    const buttons = [...message.querySelectorAll<HTMLButtonElement>('button.file-ref')]
    click(buttons[0]!)
    click(buttons[1]!)
    expect(onFileReference.mock.calls).toEqual([[{ path: 'src/a.ts', line: 3, endLine: 9 }], [{ path: 'src/lib.rs', line: 7 }]])
    expect(openUrl).not.toHaveBeenCalled()
  })

  it('a file button without a line reports its path only', () => {
    const onFileReference = vi.fn()
    const { message } = mount('[notes](NOTES.md)', { onFileReference })
    click(message.querySelector('button.file-ref')!)
    expect(onFileReference).toHaveBeenCalledWith({ path: 'NOTES.md' })
  })

  it('an external link is opened through the system, and the webview does not navigate', async () => {
    const onLinkResult = vi.fn()
    const { message } = mount('[site](https://example.com/a)', { onLinkResult })
    const event = click(message.querySelector('a')!)
    expect(event.defaultPrevented).toBe(true)
    expect(openUrl).toHaveBeenCalledWith('https://example.com/a')
    await vi.waitFor(() => expect(onLinkResult).toHaveBeenCalledWith(null))
  })

  it('a link that cannot open reports its error', async () => {
    vi.mocked(openUrl).mockRejectedValueOnce(new Error('ouverture refusée'))
    const onLinkResult = vi.fn()
    const { message } = mount('[site](https://example.com/a)', { onLinkResult })
    click(message.querySelector('a')!)
    await vi.waitFor(() => expect(onLinkResult).toHaveBeenCalledWith('ouverture refusée'))
  })

  it('a link is not opened while the text streams (linksEnabled false), but still does not navigate', () => {
    const { message } = mount('[site](https://example.com/a)', { linksEnabled: false })
    const event = click(message.querySelector('a')!)
    expect(event.defaultPrevented).toBe(true)
    expect(openUrl).not.toHaveBeenCalled()
  })

  it('a file reference is inactive while the text streams (linksEnabled false)', () => {
    const onFileReference = vi.fn()
    const { message } = mount('Voir [notes](NOTES.md) et src/lib.rs:7.', { linksEnabled: false, onFileReference })
    click(message.querySelector('button.file-ref')!)
    click(message.querySelectorAll('button.file-ref')[1]!)
    expect(onFileReference).not.toHaveBeenCalled()
  })

  it('a link with a script scheme is never opened', () => {
    const { message } = mount('texte')
    message.innerHTML = '<a href="javascript:alert(1)">piège</a>'
    const event = click(message.querySelector('a')!)
    expect(event.defaultPrevented).toBe(true)
    expect(openUrl).not.toHaveBeenCalled()
  })

  it('an in-page anchor scrolls to and focuses its target in the same conversation only', () => {
    // Another conversation container holds an element with the same id: it must not be touched.
    const other = document.createElement('div')
    other.setAttribute('data-conversation-scroll', '')
    other.innerHTML = '<h3 id="x-titre">Autre</h3>'
    document.body.append(other)
    const otherHeading = other.querySelector<HTMLElement>('h3')!
    const otherScroll = vi.fn()
    otherHeading.scrollIntoView = otherScroll

    const { scroll, message } = mount('## Titre\n\nVoir [la section](#Titre).')
    const heading = scroll.querySelector<HTMLElement>('#x-titre')!
    const scrollIntoView = vi.fn()
    heading.scrollIntoView = scrollIntoView
    const focus = vi.spyOn(heading, 'focus')

    const event = click(message.querySelector('a[data-local-anchor]')!)
    expect(event.defaultPrevented).toBe(true)
    expect(scrollIntoView).toHaveBeenCalledTimes(1)
    expect(focus).toHaveBeenCalledTimes(1)
    expect(otherScroll).not.toHaveBeenCalled()
    expect(openUrl).not.toHaveBeenCalled()
  })

  it('a footnote back link is an in-page anchor too', () => {
    const { scroll, message } = mount('Texte[^1].\n\n[^1]: note')
    const target = scroll.querySelector<HTMLElement>('[id="x-fnref:1-1"]')!
    const scrollIntoView = vi.fn()
    target.scrollIntoView = scrollIntoView
    const event = click(message.querySelector('a[data-fn-back]')!)
    expect(event.defaultPrevented).toBe(true)
    expect(scrollIntoView).toHaveBeenCalledTimes(1)
  })
})

// ─── 6. Mermaid (logic, the renderer is mocked) ──────────────────────────────

const SOURCE = 'flowchart TD\nA-->B'

describe('mermaid blocks while streaming', () => {
  it('an open fence is not complete until its closing fence arrives', () => {
    const doc = new MarkdownDocument({ idPrefix: 'm' })
    expect(doc.update('```mermaid\nflowchart TD\nA-->B', false)).toEqual([{ key: 'b0', kind: 'mermaid', source: SOURCE, complete: false }])
    expect(doc.update('```mermaid\nflowchart TD\nA-->B\n```', false)).toEqual([{ key: 'b0', kind: 'mermaid', source: SOURCE, complete: true }])
  })

})

describe('mermaid rendering', () => {
  const options = { theme: 'app' as const, mode: 'light' as const, fontFamily: 'sans-serif' }

  it('two messages rendering at once give their diagrams distinct ids', async () => {
    await Promise.all([
      renderMermaid(SOURCE, { ...options, id: 'm-alpha-b0-1' }),
      renderMermaid(SOURCE, { ...options, id: 'm-beta-b0-1' }),
    ])
    expect(mermaidApi.render.mock.calls.map((call) => call[0])).toEqual(['m-alpha-b0-1', 'm-beta-b0-1'])
  })

  it('uses the selected sandbox theme and style while neutralizing diagram overrides', async () => {
    await renderMermaid('%%{init:{"securityLevel":"loose"}}%%\nflowchart TD\nA-->B\nclick A "https://evil.test"', { theme: 'forest', look: 'handDrawn', fontFamily: 'sans-serif', id: 'm-sandbox-config' })
    expect(mermaidApi.initialize).toHaveBeenLastCalledWith(expect.objectContaining({ securityLevel: 'sandbox', theme: 'forest', look: 'handDrawn', startOnLoad: false, htmlLabels: false }))
    const source = (mermaidApi.render.mock.calls.at(-1) as unknown as [string, string])[1]
    expect(source).not.toMatch(/securityLevel|%%\{/)
    // A web link is the one interaction kept.
    expect(source).toContain('click A "https://evil.test"')
  })

  it('applies the security level chosen in the settings, and the app colours for the app theme', async () => {
    await renderMermaid(SOURCE, { theme: 'app', mode: 'dark', securityLevel: 'strict', fontFamily: 'sans-serif', id: 'm-strict-app', colors: { background: '#101010', foreground: '#f0f0f0' } })
    expect(mermaidApi.initialize).toHaveBeenLastCalledWith(expect.objectContaining({ securityLevel: 'strict', theme: 'base', themeVariables: expect.objectContaining({ background: '#101010' }) }))
  })

  it('an identifier outside the allowed pattern is refused before rendering', async () => {
    const result = await renderMermaid(SOURCE, { ...options, id: '1-bad id' })
    expect(result).toEqual({ error: 'Identifiant de diagramme invalide' })
    expect(mermaidApi.render).not.toHaveBeenCalled()
  })

  it('each message has its own cache: a hit in one is not a hit in another', async () => {
    const first = createMermaidCache()
    const second = createMermaidCache()
    const request = { ...options, id: 'mmd-1' }
    await renderMermaid(SOURCE, { ...request, cache: first })
    await renderMermaid(SOURCE, { ...request, cache: second })
    expect(mermaidApi.render).toHaveBeenCalledTimes(2)
    await renderMermaid(SOURCE, { ...request, cache: first })
    expect(mermaidApi.render).toHaveBeenCalledTimes(2)
  })

  it('a source over the bound is refused without calling the renderer', async () => {
    const result = await renderMermaid(`flowchart TD\n${'A-->B;'.repeat(MAX_SOURCE_LENGTH / 6 + 1)}`, { ...options, id: 'mmd-big' })
    expect(result).toHaveProperty('error')
    expect((result as { error: string }).error).toMatch(/trop long/)
    expect(mermaidApi.render).not.toHaveBeenCalled()
  })
})

// ─── 7. Proposed diffs ───────────────────────────────────────────────────────

function diffsOf(blocks: RenderedBlock[]): unknown[] {
  return blocks.flatMap((block) => (block.kind === 'html' ? [] : [[block.kind, block.source, block.complete]]))
}

describe('proposed diffs', () => {
  it('a top-level diff or patch fence is a read-only diff block', () => {
    const blocks = renderReference('```diff\n+a\n-b\n```\n\n```patch\n-c\n```', { idPrefix: 'd' })
    expect(diffsOf(blocks)).toEqual([
      ['diff', '+a\n-b', true],
      ['diff', '-c', true],
    ])
  })

  it('a diff fence inside a list is a code block', () => {
    const blocks = renderReference('- élément\n\n  ```diff\n  +a\n  ```', { idPrefix: 'd' })
    expect(diffsOf(blocks)).toEqual([])
    expect(htmlOf(blocks)).toContain('<span class="inline-flex min-w-0 items-center gap-1.5 font-mono">diff</span>')
  })

  it('a multi-file proposal: each file has its own path, binary flag and counts', () => {
    const proposal = [
      'diff --git a/src/a.ts b/src/a.ts',
      '--- a/src/a.ts',
      '+++ b/src/a.ts',
      '@@ -1,2 +1,3 @@',
      '-old',
      '+new',
      '+extra',
      ' keep',
      'diff --git a/img.png b/img.png',
      'Binary files a/img.png and b/img.png differ',
      'diff --git a/notes.md b/notes.md',
      '--- a/notes.md',
      '+++ b/notes.md',
      '@@ -1 +1 @@',
      '-x',
      '+y',
    ].join('\n')
    expect(patchFiles(proposal).map((file) => [file.path, file.additions, file.deletions, file.binary])).toEqual([
      ['src/a.ts', 2, 1, false],
      ['img.png', 0, 0, true],
      ['notes.md', 1, 1, false],
    ])
  })

  it('a truncated hunk counts the lines it received', () => {
    const [file] = patchFiles('--- a/x\n+++ b/x\n@@ -1,3 +1,3 @@\n-a\n+b\n')
    expect(file).toMatchObject({ path: 'x', additions: 1, deletions: 1 })
  })

  it('a proposal without any hunk header counts its bare lines per file', () => {
    expect(patchFiles('--- a/x\n+++ b/x\n+one\n--- a/y\n+++ b/y\n+two\n').map((file) => [file.path, file.additions])).toEqual([
      ['x', 1],
      ['y', 1],
    ])
  })

  it('(défaut) a removed line "-- x" and an added line "++ y" of a hunk are not a new file', () => {
    // Inside a hunk, `--- old` is a removed line and `+++ new` an added one: one file, one of each.
    const files = patchFiles('--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n--- old\n+++ new\n ctx\n')
    expect(files.map((file) => [file.path, file.additions, file.deletions])).toEqual([['x', 1, 1]])
  })
})

// ─── 8. A stream that ends in the middle of a block ──────────────────────────

describe('a final stream keeps the text it received', () => {
  const cases: Array<[string, string, string[]]> = [
    ['fence', '```ts\nconst valeur = 42;\nlet texte = "abc', ['const valeur = 42;', 'let texte = "abc']],
    ['formula', 'Avant.\n\n$$\n\\sum_{i=1}^n i', ['\\sum_{i=1}^n i']],
    ['table', '| nom | valeur |\n|---|---|\n| alpha | 12', ['nom', 'valeur', 'alpha', '12']],
    ['note', 'Texte[^1].\n\n[^1]: Définition en cou', ['Définition en cou']],
    ['alert', '> [!NOTE]\n> Attention en cours', ['Attention en cours']],
    ['heading', '## Titre en cou', ['Titre en cou']],
  ]

  it.each(cases)('an open %s', (_name, source, received) => {
    const doc = new MarkdownDocument({ idPrefix: 't' })
    const text = textOf(htmlOf(doc.update(source, true)))
    for (const piece of received) expect(text).toContain(piece)
  })

  it('a replaced source (history reloaded, shorter text) is rendered from scratch', () => {
    const doc = new MarkdownDocument({ idPrefix: 'r' })
    doc.update('Un long paragraphe, avec [^1].\n\n[^1]: note\n\n## Titre', false)
    expect(htmlOf(doc.update('Un', false))).toBe(render('Un', 'r'))
    expect(doc.headings).toEqual([])
  })

  it('after dispose, an update returns nothing and a second dispose is harmless', () => {
    const doc = new MarkdownDocument({ idPrefix: 'd' })
    doc.update('## A\n\ntexte', false)
    doc.dispose()
    expect(doc.update('## B', true)).toEqual([])
    doc.dispose()
    expect(doc.headings).toEqual([])
  })
})
