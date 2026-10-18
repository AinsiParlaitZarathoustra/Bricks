// Sprint 14.5 regression check in a real DOM (Chromium headless shell): ordinary Markdown rendered by
// the frozen one-shot renderer (tests/bench/baseline) and by the block renderer (renderReference).
// Built by tests/bench/browser/vite.config.ts; run by tests/bench/browser/run.sh (dump-dom).
// Only the DOM differs from the Node check: DOMPurify here is the browser's own.
import { renderMarkdown as renderBaseline } from '../baseline/markdown-baseline.ts'
import { MarkdownDocument, renderReference } from '../../../src/lib/features/conversation/markdown/document'
import { deltaSplits, type SplitMode } from './splits'
import mixedLarge from '../../fixtures/markdown/mixed-large.md?raw'
import simple from '../../fixtures/markdown/simple.md?raw'
import rustLong from '../../fixtures/markdown/rust-long.md?raw'
import nested from '../../fixtures/markdown/nested.md?raw'
import tables from '../../fixtures/markdown/tables.md?raw'
import hostile from '../../fixtures/markdown/hostile.md?raw'
import lateDefs from '../../fixtures/markdown/late-defs.md?raw'
import alerts from '../../fixtures/markdown/alerts.md?raw'
import mathFx from '../../fixtures/markdown/math.md?raw'
import filesDiff from '../../fixtures/markdown/files-diff.md?raw'
import mermaid from '../../fixtures/markdown/mermaid.md?raw'

const hostileOrdinary = hostile.replace(/```mermaid[\s\S]*?```/, '').replace(/^Formule KaTeX piégée.*$/m, '')

const ORDINARY: Record<string, string> = {
  simple,
  'rust-long': rustLong,
  nested,
  tables,
  'gfm-emphasis': '**gras** *italique* ~~barré~~ ***les deux*** `code` et \\*échappé\\* et _souligné_.',
  'gfm-tasks': '- [ ] Tâche non cochée\n- [x] Tâche cochée\n  - [ ] Sous-tâche\n',
  'gfm-aligned-table': '| a | b | c |\n|:--|:-:|--:|\n| 1 | 2 | 3 |\n| longue cellule | x | 4,5 |\n',
  'gfm-www-autolink': 'Voir www.example.org/page et https://example.org/x. Mail : support@example.org.',
  'gfm-external-address': '[le site](https://example.org/a) et [https://example.org/b](https://example.org/b) et [](https://example.org/c).',
  'raw-html-escaped': '<b>gras</b> et <script>x()</script> <img src=x onerror=alert(1)> <span title="t">s</span>',
  'images-as-text': '![logo](https://example.org/l.png) et ![](x.png) et ![alt sans url]()',
  'shifted-headings': '# Un\n\n## Deux\n\n### Trois\n\n###### Six\n\nTexte après.',
  'code-langs': '```ts\nconst a: number = 1\n```\n\n```\nsans langue\n```\n\n```foo\nlangue inconnue <b>\n```\n\n~~~python extra words\nprint("x")\n~~~\n',
  'inline-code-html': 'Le tag `<b>x</b>` reste du code, et `a && b` aussi.',
  'links-safe-schemes': '[mail](mailto:a@example.org) [js](javascript:alert(1)) [data](data:text/html,xx) [vide]()',
  'hard-breaks': 'ligne 1\nligne 2\n\nNouveau paragraphe.',
  blockquote: '> citation\n> suite\n>\n> > imbriquée',
  'ordered-start': '3. troisième\n4. quatrième\n\n---\n\nFin.',
  'dollar-amounts': 'Il coûte 5 $ ou 10 $. Ou $5 et $10 au total, et 2 € ailleurs.',
  'unresolved-footnote-like': 'Texte [^1] sans définition, et [^nom] aussi.',
  'bare-names': 'Le fichier package.json et la version v1.2.3, e.g. le test, example.com.',
  'hostile-ordinary': hostileOrdinary,
}

const NEW_SYNTAX: Record<string, string> = {
  'late-defs (footnotes, anchors)': lateDefs,
  alerts,
  math: mathFx,
  'files-diff (file refs, diff)': filesDiff,
  mermaid,
}

/** Canonical DOM: whitespace-only text outside code dropped; ids of message headings dropped. */
function canon(html: string): string[] {
  const root = document.createElement('div')
  root.innerHTML = html
  const out: string[] = []
  const walk = (node: Node, inCode: boolean) => {
    for (const child of Array.from(node.childNodes)) {
      if (child.nodeType === 3) {
        const text = child.textContent ?? ''
        if (!inCode && text.trim() === '') continue
        out.push(`T:${text}`)
      } else if (child.nodeType === 1) {
        const el = child as Element
        const tag = el.tagName.toLowerCase()
        const attrs = Array.from(el.attributes)
          .filter((a) => !(/^h[1-6]$/.test(tag) && a.name === 'id' && a.value.startsWith('md-')))
          .map((a) => `${a.name}="${a.value}"`)
          .sort()
        out.push(`<${tag}${attrs.length ? ` ${attrs.join(' ')}` : ''}>`)
        walk(el, inCode || tag === 'pre' || tag === 'code')
        out.push(`</${tag}>`)
      }
    }
  }
  walk(root, false)
  return out
}

function firstDiff(a: string[], b: string[]) {
  const max = Math.max(a.length, b.length)
  for (let i = 0; i < max; i++) {
    if (a[i] !== b[i]) {
      return { index: i, baseline: a.slice(Math.max(0, i - 6), i + 8).join(' '), reference: b.slice(Math.max(0, i - 6), i + 8).join(' ') }
    }
  }
  return null
}

/** Multiset difference of two canonical lists: items only on one side, with their counts (first 12 of each). */
function multisetDiff(a: string[], b: string[]) {
  const count = (list: string[]) => {
    const m = new Map<string, number>()
    for (const item of list) m.set(item, (m.get(item) ?? 0) + 1)
    return m
  }
  const ca = count(a)
  const cb = count(b)
  const only = (x: Map<string, number>, y: Map<string, number>) =>
    [...x.entries()].filter(([k, v]) => v > (y.get(k) ?? 0)).map(([k, v]) => `${k} x${v - (y.get(k) ?? 0)}`).slice(0, 12)
  return { onlyBaseline: only(ca, cb), onlyReference: only(cb, ca) }
}

/** Same canonical form, without the `type="button"` that the baseline's DOMPurify drops from the copy buttons. */
function canonNoButtonType(list: string[]) {
  return list.map((item) => item.replace(/^<button( [^>]*)?>$/, (m) => m.replace(/ type="button"/, '')))
}

function compare(source: string) {
  const baseline = renderBaseline(source)
  const blocks = renderReference(source, { idPrefix: 'md' })
  const reference = blocks.map((b) => (b.kind === 'html' ? b.html : '')).join('')
  const a = canon(baseline)
  const b = canon(reference)
  const diff = firstDiff(a, b)
  const diffNoType = firstDiff(canonNoButtonType(a), canonNoButtonType(b))
  return {
    equal: diff === null,
    equalModuloCopyButtonType: diffNoType === null,
    nonHtmlBlocks: blocks.filter((x) => x.kind !== 'html').length,
    baselineItems: a.length,
    referenceItems: b.length,
    diff,
    diffModuloCopyButtonType: diffNoType,
    multiset: multisetDiff(canonNoButtonType(a), canonNoButtonType(b)),
  }
}

// Boundary probes: ordinary-looking text next to the new syntaxes (reported, not counted as equivalence).
const PROBES: Record<string, string> = {
  'probe-url-with-line': 'Adresse : https://x.y/src/a.rs:3 et www.example.org/src/lib.rs:4.',
  'probe-relative-link-html': 'Voir [le guide](guide.html) et [notes](NOTES.md).',
  'probe-relative-link-dir': 'Voir [la section](docs/guide.md#install).',
  'probe-windows-path': 'Chemin C:\\Users\\dev\\main.rs:10 et e.g. src/main.rs.',
  'probe-escaped-bracket-math': 'Le motif \\[ ceci \\] et \\( cela \\) reste du texte.',
  'probe-footnote-real': 'Note[^a] ici.\n\n[^a]: La note.',
  'probe-local-anchor': '## Titre\n\nVoir [le titre](#titre).',
}

/** Final incremental update (streamed by deltas, then update(text, true)) against the one-shot block rendering, 5 splits. */
const MODES: SplitMode[] = ['char1', 'n7', 'n64', 'boundaries', 'random(1)']
const CORPUS: Record<string, string> = {
  simple,
  'rust-long': rustLong,
  nested,
  tables,
  'late-defs': lateDefs,
  math: mathFx,
  mermaid,
  alerts,
  'files-diff': filesDiff,
  hostile,
  'mixed-large': mixedLarge,
}
function incremental(source: string) {
  const expected = JSON.stringify(renderReference(source, { idPrefix: 'md' }))
  const mismatches: string[] = []
  // A stream step whose stats say the fallback was used (source shown as one paragraph) is a failure.
  const fallbacks: string[] = []
  const started = performance.now()
  for (const mode of MODES) {
    const doc = new MarkdownDocument({ idPrefix: 'md' })
    let prefix = ''
    let steps = 0
    for (const delta of deltaSplits(source, mode)) {
      prefix += delta
      steps += 1
      doc.update(prefix, false)
      if (doc.stats.fallback) {
        fallbacks.push(`${mode}@${steps}`)
        break
      }
    }
    const final = JSON.stringify(doc.update(source, true))
    if (doc.stats.fallback) fallbacks.push(`${mode}:final`)
    doc.dispose()
    if (final !== expected) mismatches.push(mode)
  }
  return { mismatches, fallbacks: fallbacks.slice(0, 6), ms: Math.round(performance.now() - started) }
}

/** Security probe on the new pipeline's output, read back through the DOM (nothing is loaded or run). */
function securityOf(html: string) {
  const root = document.createElement('div')
  root.innerHTML = html
  const elements = [...root.querySelectorAll('*')]
  const attributes = elements.flatMap((el) => [...el.attributes].map((a) => ({ tag: el.tagName.toLowerCase(), name: a.name.toLowerCase(), value: a.value })))
  const tags = new Set(elements.map((el) => el.tagName.toLowerCase()))
  const dangerous = ['script', 'img', 'svg', 'foreignobject', 'iframe', 'form', 'input', 'object', 'embed', 'style', 'link', 'meta', 'base', 'select', 'textarea']
  return {
    dangerousTags: dangerous.filter((t) => tags.has(t)),
    eventHandlers: attributes.filter((a) => a.name.startsWith('on')).map((a) => `${a.tag}@${a.name}`),
    unsafeUris: attributes
      .filter((a) => ['href', 'src', 'xlink:href', 'action', 'formaction'].includes(a.name) && !/^(https?:|mailto:|#)/i.test(a.value))
      .map((a) => `${a.tag}@${a.name}=${a.value.slice(0, 40)}`),
    styleAttributes: attributes.filter((a) => a.name === 'style').length,
    buttonTypesNotButton: attributes.filter((a) => a.tag === 'button' && a.name === 'type' && a.value !== 'button').length,
  }
}
const SECURITY_SOURCES: Record<string, string> = { 'hostile (full, with mermaid and KaTeX)': hostile, 'hostile-ordinary': hostileOrdinary }
const security = Object.fromEntries(
  Object.entries(SECURITY_SOURCES).map(([name, source]) => [name, securityOf(renderReference(source, { idPrefix: 'md' }).map((b) => (b.kind === 'html' ? b.html : '')).join(''))]),
)

/** Ordinary messages need no math renderer: KaTeX is loaded only when a block has a formula. */
const needsMath = Object.fromEntries(
  Object.entries(ORDINARY).map(([name, source]) => {
    const doc = new MarkdownDocument({ idPrefix: 'md' })
    doc.update(source, true)
    const value = doc.needsMath
    doc.dispose()
    return [name, value]
  }),
)

const result: Record<string, unknown> = {
  security,
  needsMathOrdinary: Object.values(needsMath).filter(Boolean).length,
  needsMathCases: Object.entries(needsMath).filter(([, v]) => v).map(([k]) => k),
  incremental: Object.fromEntries(
    Object.entries(CORPUS).map(([name, source]) => {
      try {
        return [name, incremental(source)]
      } catch (error) {
        return [name, { error: String(error), stack: error instanceof Error ? error.stack?.slice(0, 600) : '' }]
      }
    }),
  ),
  probes: Object.fromEntries(Object.entries(PROBES).map(([name, source]) => [name, { ...compare(source), baseline: renderBaseline(source), html: renderReference(source, { idPrefix: 'md' }).map((b) => (b.kind === 'html' ? b.html : `[${b.kind}]`)).join('') }])),
  userAgent: navigator.userAgent,
  domPurifySanity: (() => {
    const probe = document.createElement('div')
    probe.innerHTML = '<p>a</p>\n<p>b</p>'
    return probe.innerHTML
  })(),
  ordinary: Object.fromEntries(Object.entries(ORDINARY).map(([name, source]) => [name, compare(source)])),
  newSyntax: Object.fromEntries(Object.entries(NEW_SYNTAX).map(([name, source]) => [name, compare(source)])),
}

// Written as a JSON script element: escaped so that no `</` in the content can close it.
const json = JSON.stringify(result).replace(/<\//g, '<\\/')
const out = document.createElement('script')
out.type = 'application/json'
out.id = 'result'
out.textContent = json
document.body.appendChild(out)
