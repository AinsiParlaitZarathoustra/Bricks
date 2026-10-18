// @vitest-environment happy-dom
// Bricks syntax of assistant messages: math, alerts, footnotes, file references, anchors,
// heading slugs. Pure parsers are checked directly; rendering goes through renderReference.
import { describe, expect, it, vi } from 'vitest'
import { findFileReferences, parseFileReference } from '../../src/lib/features/conversation/markdown/fileref-parse'
import { renderReference } from '../../src/lib/features/conversation/markdown/document'
import { slugify } from '../../src/lib/features/conversation/markdown/extensions'
import type { RenderedBlock } from '../../src/lib/features/conversation/markdown/types'

// happy-dom drops the first element of an HTML fragment inside DOMPurify (a test-only DOM
// quirk). Sanitizing is covered by the sanitizer tests; here the HTML is compared as written.
vi.mock('../../src/lib/features/conversation/markdown/sanitize', () => ({
  sanitizeMarkdown: (html: string) => html,
}))

function htmlOf(source: string, math?: (tex: string, display: boolean) => { html: string } | { error: string }): string {
  const blocks: RenderedBlock[] = renderReference(source, { idPrefix: 'x', math: math ? (tex, display) => math(tex, display) : null })
  return blocks.map((block) => (block.kind === 'html' ? block.html : `[${block.kind}:${block.source}]`)).join('')
}

describe('parseFileReference (link targets)', () => {
  it('reads a project file with a line fragment', () => {
    expect(parseFileReference('src/lib.rs#L42', 'href')).toEqual({ path: 'src/lib.rs', line: 42 })
    expect(parseFileReference('src/a.ts#L3-L9', 'href')).toEqual({ path: 'src/a.ts', line: 3, endLine: 9 })
    expect(parseFileReference('src/a.ts#L3C2-L9C4', 'href')).toEqual({ path: 'src/a.ts', line: 3, endLine: 9 })
  })

  it('decodes escapes and accepts a line suffix', () => {
    expect(parseFileReference('src/a%20b.ts', 'href')).toEqual({ path: 'src/a b.ts' })
    expect(parseFileReference('src/a.ts:12', 'href')).toEqual({ path: 'src/a.ts', line: 12 })
    expect(parseFileReference('./src/a.ts', 'href')).toEqual({ path: 'src/a.ts' })
  })

  it('refuses schemes, absolute and in-page targets, parent segments and malformed escapes', () => {
    for (const href of ['https://x.y/a.ts', 'mailto:a@b.c', 'file:///etc/passwd', '/etc/a.ts', '//host/a.ts', '#titre', '../a.ts', 'a/../b.ts', '%E0%A4%A.ts', 'a/b.ts?x=1']) {
      expect(parseFileReference(href, 'href'), href).toBeNull()
    }
  })

  it('accepts a bare file name as a link target, but not a host name', () => {
    expect(parseFileReference('NOTES.md', 'href')).toEqual({ path: 'NOTES.md' })
    expect(parseFileReference('example.com', 'href')).toBeNull()
    expect(parseFileReference('www.example.org', 'href')).toBeNull()
    expect(parseFileReference('www.example.com/a.html', 'href')).toBeNull()
    expect(parseFileReference('example.com/index.html', 'href')).toBeNull()
    expect(parseFileReference('docs/', 'href')).toBeNull()
    expect(parseFileReference('src/README', 'href')).toBeNull()
  })
})

describe('bare file references in text', () => {
  it('finds paths with a line or a range, and dot-led paths', () => {
    const found = findFileReferences('Voir src/lib.rs:42-50, puis ./a/b.ts et .github/ci.yml.')
    expect(found.map((match) => match.ref)).toEqual([
      { path: 'src/lib.rs', line: 42, endLine: 50 },
      { path: 'a/b.ts' },
      { path: '.github/ci.yml' },
    ])
    expect(found[0]?.start).toBe(5)
  })

  it('needs a directory or a line in text: names, versions, addresses and URLs stay text', () => {
    expect(parseFileReference('lib.rs', 'text')).toBeNull()
    const text = 'lib.rs seul, example.com/index.html, www.a.com/x.ts, https://a.b/c/d.ts, mail a@b.c, 1.5/2.3, e.g. ok'
    expect(findFileReferences(text)).toEqual([])
  })

  it('a Windows, home or Unix absolute path is never a reference', () => {
    expect(findFileReferences('Chemin C:\\Users\\dev\\main.rs:10 ici.')).toEqual([])
    expect(findFileReferences('Voir ~/src/a.ts:3 et /home/dev/a.rs:3.')).toEqual([])
    expect(findFileReferences('Dans src\\lib.rs:4 seulement.')).toEqual([])
    expect(parseFileReference('~/src/a.ts', 'href')).toBeNull()
    expect(parseFileReference('C:\\Users\\a.ts', 'href')).toBeNull()
    const html = htmlOf('Chemin C:\\Users\\dev\\main.rs:10')
    expect(html).not.toContain('file-ref')
  })

  it('does not take a trailing full stop', () => {
    expect(findFileReferences('Fichier src/a.ts.')[0]?.ref).toEqual({ path: 'src/a.ts' })
  })
})

describe('heading slugs', () => {
  it('lowercases, removes accents and punctuation, keeps letters and digits, dashes each space', () => {
    expect(slugify('Élève & Café 2')).toBe('eleve--cafe-2')
    expect(slugify('Hello, World!')).toBe('hello-world')
    expect(slugify('Ça—ne 日本')).toBe('cane-日本')
  })

  it('falls back to "section" when nothing is left', () => {
    expect(slugify('???')).toBe('section')
    expect(slugify('—')).toBe('section')
  })
})

describe('math', () => {
  it('leaves dollar amounts as text (pandoc rule)', () => {
    const html = htmlOf('Prix : $5 et $10.\n\nIl coûte 5$ ou 10$.')
    expect(html).toContain('$5 et $10')
    expect(html).toContain('5$ ou 10$')
    expect(html).not.toContain('math-src')
  })

  it('an escaped dollar is a literal dollar', () => {
    const html = htmlOf('Coût \\$5 et \\$x\\$')
    expect(html).not.toContain('math-src')
    expect(html).toContain('$5')
  })

  it('reads inline, display and bracket forms with their delimiters', () => {
    const html = htmlOf('$x^2$ et $$a+b$$ et \\(\\alpha\\) et \\[\\int\\]\n\n$$\n\\sum i\n$$\n')
    expect(html).toContain('<span class="math-src" data-display="0">$x^2$</span>')
    expect(html).toContain('<span class="math-src" data-display="1">$$a+b$$</span>')
    expect(html).toContain('<span class="math-src" data-display="0">\\(\\alpha\\)</span>')
    expect(html).toContain('<span class="math-src" data-display="1">\\[\\int\\]</span>')
    expect(html).toContain('<span class="math-src" data-display="1">$$\\sum i$$</span>')
  })

  it('an unclosed expression stays text', () => {
    const html = htmlOf('Un $x qui continue\n\n$$\nsans fin')
    expect(html).not.toContain('math-src')
    expect(html).toContain('$x qui continue')
  })

  it('is not interpreted in code spans and fences', () => {
    const html = htmlOf('Code `$x$` ici.\n\n```\n$$y$$\n```\n')
    expect(html).not.toContain('math-src')
    expect(html).toContain('<code>$x$</code>')
    expect(html).toContain('$$y$$')
  })

  it('goes to the renderer, with the display flag, or shows an error as text', () => {
    const calls: Array<[string, boolean]> = []
    const html = htmlOf('$x$ et $$y$$', (tex, display) => {
      calls.push([tex, display])
      return { html: `<i>${tex}</i>` }
    })
    expect(calls).toEqual([
      ['x', false],
      ['y', true],
    ])
    expect(html).toContain('<i>x</i>')
    const failed = htmlOf('$z$', () => ({ error: 'boom' }))
    expect(failed).toContain('<span class="math-error" title="boom">$z$</span>')
  })
})

describe('GitHub alerts', () => {
  it('turns a blockquote opened by a marker line into an alert', () => {
    const html = htmlOf('> [!warning]\n> Attention **ici**.\n\nSuite')
    expect(html).toContain('<div class="md-alert md-alert-warning" role="note">')
    expect(html).toContain('<p class="md-alert-title" data-alert-icon="warning">Attention</p>')
    expect(html).toContain('Attention <strong>ici</strong>.')
  })

  it('titles the five kinds in French', () => {
    const titles = ['NOTE', 'TIP', 'IMPORTANT', 'WARNING', 'CAUTION'].map((kind) => htmlOf(`> [!${kind}]\n> x`))
    expect(titles[0]).toContain('>Note</p>')
    expect(titles[1]).toContain('>Astuce</p>')
    expect(titles[2]).toContain('>Important</p>')
    expect(titles[3]).toContain('>Attention</p>')
    expect(titles[4]).toContain('>Danger</p>')
  })

  it('a marker that shares its line is an ordinary blockquote', () => {
    const html = htmlOf('> [!NOTE] texte sur la même ligne')
    expect(html).not.toContain('md-alert')
    expect(html).toContain('<blockquote>')
  })

  it('is not recognized inside a code block', () => {
    const html = htmlOf('```\n> [!NOTE]\n> x\n```\n')
    expect(html).not.toContain('md-alert')
  })
})

describe('footnotes', () => {
  it('numbers references in order and lists the notes with back links', () => {
    const html = htmlOf('Premier[^b] puis[^a] et encore[^b].\n\n[^a]: Note A\n\n[^b]: Note B\n')
    expect(html).toContain('<sup class="fn-ref"><a href="#x-fn:b" id="x-fnref:b-1" data-fn>1</a></sup>')
    expect(html).toContain('<sup class="fn-ref"><a href="#x-fn:a" id="x-fnref:a-1" data-fn>2</a></sup>')
    expect(html).toContain('<sup class="fn-ref"><a href="#x-fn:b" id="x-fnref:b-2" data-fn>1</a></sup>')
    expect(html.indexOf('id="x-fn:b"')).toBeLessThan(html.indexOf('id="x-fn:a"'))
    expect(html).toContain('href="#x-fnref:b-1"')
    expect(html).toContain('href="#x-fnref:b-2"')
  })

  it('reads indented continuation lines of a definition', () => {
    const html = htmlOf('Texte[^n].\n\n[^n]: première\n    seconde ligne\n\nFin\n')
    expect(html).toContain('première')
    expect(html).toContain('seconde ligne')
    expect(html).toContain('Fin')
  })

  it('is not recognized in code', () => {
    const html = htmlOf('`[^1]` et\n\n```\n[^1]: pas une note\n```\n')
    expect(html).not.toContain('md-footnotes')
    expect(html).toContain('[^1]: pas une note')
  })
})

describe('file references in messages', () => {
  it('a link to a project file is a file button with its line range', () => {
    const html = htmlOf('Voir [lib.rs:42](src/lib.rs#L42) et [a](src/a%20b.ts#L3-L9).')
    expect(html).toContain('<button type="button" class="file-ref" data-file-path="src/lib.rs" data-line="42">lib.rs:42</button>')
    expect(html).toContain('<button type="button" class="file-ref" data-file-path="src/a b.ts" data-line="3" data-end-line="9">a</button>')
  })

  it('a bare path in text is a file button, not in code or in a URL', () => {
    const html = htmlOf('Fichier src/lib.rs:7 ; `src/lib.rs:7` ; https://example.com/src/lib.rs:7')
    expect(html).toContain('<button type="button" class="file-ref" data-file-path="src/lib.rs" data-line="7">src/lib.rs:7</button>')
    expect(html.match(/class="file-ref"/g)).toHaveLength(1)
    expect(html).toContain('<code>src/lib.rs:7</code>')
  })

  it('a link to a bare file name is a file button, a link to a host name is not', () => {
    const html = htmlOf('[notes](NOTES.md) et [site](example.com)')
    expect(html).toContain('data-file-path="NOTES.md">notes</button>')
    expect(html).not.toContain('data-file-path="example.com"')
  })

  it('a link to a URL keeps its link', () => {
    const html = htmlOf('[site](https://example.com/a.ts)')
    expect(html).toContain('href="https://example.com/a.ts"')
    expect(html).not.toContain('file-ref')
  })
})

describe('anchors', () => {
  it('a link to a heading of the message points to its id', () => {
    const html = htmlOf('# Les résultats\n\nVoir [la section](#les-resultats) et [autre](#Les%20r%C3%A9sultats).')
    expect(html).toContain('<h2 id="x-les-resultats">Les résultats</h2>')
    expect(html).toContain('<a href="#x-les-resultats"')
    expect(html.match(/href="#x-les-resultats"/g)).toHaveLength(2)
  })

  it('duplicate headings get -1, -2 in document order', () => {
    const html = htmlOf('## Notes\n\nA\n\n## Notes\n\nB\n\n## Notes\n')
    expect(html).toContain('id="x-notes"')
    expect(html).toContain('id="x-notes-1"')
    expect(html).toContain('id="x-notes-2"')
  })
})
