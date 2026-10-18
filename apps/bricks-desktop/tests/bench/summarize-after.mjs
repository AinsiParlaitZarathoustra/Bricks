#!/usr/bin/env node
// Writes results/after.md: reference (c1fe3de, run-baseline.mjs) against the block pipeline (run-after.mjs),
// and the thresholds of THRESHOLDS.md checked against the measured numbers. Usage:
//   node tests/bench/summarize-after.mjs results/after-<date>.json
import { readFileSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const BENCH = dirname(fileURLToPath(import.meta.url))
const RESULTS = join(BENCH, 'results')
const afterFile = process.argv[2]
if (!afterFile) throw new Error('usage: node tests/bench/summarize-after.mjs <after-json>')
const after = JSON.parse(readFileSync(afterFile, 'utf8'))
const baseline = JSON.parse(readFileSync(join(RESULTS, 'baseline-2026-10-18.json'), 'utf8'))
const ref = Object.fromEntries(baseline.fixtures.map((f) => [f.name, f]))
const now = Object.fromEntries(after.fixtures.filter((f) => f.cold).map((f) => [f.name, f]))
const independent = after.fixtures.find((f) => f.name === 'independent-additions')

const ms = (x) => (Number.isFinite(x) ? x.toFixed(x < 10 ? 2 : 1) : 'n/a')
const pct = (x) => (Number.isFinite(x) ? `${(x * 100).toFixed(0)} %` : 'n/a')
const verdict = (ok) => (ok ? 'atteint' : 'non atteint')

const lines = []
lines.push('# Mesures avant / après : pipeline Markdown Sprint 14.5')
lines.push('')
lines.push(
  `Mesure du ${after.meta.date} (après : ${after.meta.git}). ${after.meta.cpu}, ${after.meta.cpuCount} cœurs, ${after.meta.memoryGB} Go, ${after.meta.node}.`,
)
lines.push(
  `Node + happy-dom ${after.meta.versions['happy-dom']} (DOM simulé, pas la WebView). Même harnais et mêmes fixtures que la référence (${baseline.meta.date}) : ` +
    `throttle ${after.meta.throttle.tickMs} ms, ${after.meta.throttle.charsPerTick} car./${after.meta.throttle.tickMs} ms, deltas \`${after.meta.throttle.streamSplit}\`, passes chaudes = médiane de ${after.meta.iterations.hot}.`,
)
lines.push(
  'Après : un MarkdownDocument par message, un update(préfixe, false) par tick, puis update(texte, true) en fin de flux. ' +
    'Référence : un rendu complet par tick (renderMarkdown). Les HTML de la référence sont ceux de happy-dom : la 1re balise de chaque sortie y disparaît (bogue DOMPurify sous happy-dom, voir le rapport), ce qui ne change pas les temps comparés ici.',
)
lines.push('')
lines.push('## Tableau comparatif (passes chaudes sauf mention)')
lines.push('')
lines.push(
  '| Fixture | Rendus (throttle) | Plein rendu réf. / après (ms) | Total throttle réf. / après (ms) | Tick p50 / p95 réf. (ms) | Tick p50 / p95 après (ms) | Tick max après (ms) | hljs par passe réf. / après | Blocs rendus / réutilisés par passe (après) | Ticks full lex / fallback (après) | Δ tas après (MB) | Un delta à la fois, cold p50 / p95 (ms, après) | Final = référence |',
)
lines.push('| --- | --: | --: | --: | --: | --: | --: | --: | --: | --: | --: | --: | :-: |')
for (const name of Object.keys(ref)) {
  const r = ref[name].hot
  const a = now[name]?.hot
  if (!a) {
    lines.push(`| ${name} | — | ${ms(r.oneShotMs)} / — | ${ms(r.throttled.totalMs)} / — | ${ms(r.throttled.tickP50Ms)} / ${ms(r.throttled.tickP95Ms)} | non mesuré | | | | | | | |`)
    continue
  }
  const cold = now[name].cold.perDelta
  lines.push(
    `| ${name} | ${a.throttled.renders} | ${ms(r.oneShotMs)} / ${ms(a.oneShotMs)} | ${ms(r.throttled.totalMs)} / ${ms(a.throttled.totalMs)} | ` +
      `${ms(r.throttled.tickP50Ms)} / ${ms(r.throttled.tickP95Ms)} | ${ms(a.throttled.tickP50Ms)} / ${ms(a.throttled.tickP95Ms)} | ${ms(a.throttled.tickMaxMs)} | ` +
      `${ref[name].hot.throttled.hljsCallsPerPass} / ${a.throttled.hljsCallsWrapper} (doc: ${a.throttled.hljsCallsDocStats}) | ` +
      `${a.throttled.renderedBlocksPerPass} / ${a.throttled.reusedBlocksPerPass} | ${a.throttled.fullLexTicks} / ${a.throttled.fallbackTicks} | ` +
      `${ms(a.throttled.heapDeltaMB)} | ${cold ? `${ms(cold.tickP50Ms)} / ${ms(cold.tickP95Ms)}` : 'n/a'} | ` +
      `${now[name].finalEqualsReference ? 'oui' : '**NON**'} |`,
  )
}
lines.push('')
lines.push(
  'Lecture : « Un delta à la fois » est une séquence incrémentale complète sans throttle (un update par delta, passe froide seulement) : ' +
    'ce n’est pas la colonne « un rendu par delta » de la référence (rendus complets échantillonnés), elle ne se compare donc pas directement.',
)
lines.push('')
lines.push('## Ajouts indépendants (synthétique : bloc Rust, puis 300 paragraphes ajoutés un par un)')
lines.push('')
if (independent) {
  lines.push(`- Appels highlight.js après le gel du bloc (tick 6 à 300) : **${independent.highlightCallsAfterFreeze}**`)
  lines.push(`- Blocs rendus par ajout : **${ms(independent.renderedBlocksPerAddition)}** (le seul nouveau paragraphe)`)
  lines.push(`- Objets de blocs figés remplacés (sans invalidation) : **${independent.frozenReplacedTotal}**`)
  lines.push(`- Tick p95 de l'ajout : ${ms(independent.tickP95Ms)} ms`)
}
lines.push('')
lines.push('## Seuils de THRESHOLDS.md')
lines.push('')
lines.push('| Critère | Référence | Seuil | Après | Verdict |')
lines.push('| --- | --: | --: | --: | :-- |')
const m = now['mixed-large']?.hot.throttled
const rl = now['rust-long']?.hot.throttled
const worstP95 = Math.max(...Object.values(now).map((f) => f.hot.throttled.tickP95Ms))
const worstName = Object.values(now).reduce((best, f) => (f.hot.throttled.tickP95Ms > (best.hot.throttled.tickP95Ms ?? -1) ? f : best)).name
const rows = [
  ['Tick p95, mixed-large', `${ms(ref['mixed-large'].hot.throttled.tickP95Ms)} ms`, '≤ 16 ms', `${ms(m?.tickP95Ms)} ms`, verdict(m?.tickP95Ms <= 16)],
  ['Tick p95, rust-long', `${ms(ref['rust-long'].hot.throttled.tickP95Ms)} ms`, '≤ 8 ms', `${ms(rl?.tickP95Ms)} ms`, verdict(rl?.tickP95Ms <= 8)],
  ['Tick p95, chaque fixture (pire)', '—', '≤ 16 ms', `${ms(worstP95)} ms (${worstName})`, verdict(worstP95 <= 16)],
  ['Temps total streaming, mixed-large', `${ms(ref['mixed-large'].hot.throttled.totalMs)} ms`, '≤ 8 358 ms', `${ms(m?.totalMs)} ms (${pct((m?.totalMs ?? NaN) / ref['mixed-large'].hot.throttled.totalMs)} de la réf.)`, verdict(m?.totalMs <= 8358)],
  ['Temps total streaming, rust-long', `${ms(ref['rust-long'].hot.throttled.totalMs)} ms`, '≤ 1 428 ms', `${ms(rl?.totalMs)} ms (${pct((rl?.totalMs ?? NaN) / ref['rust-long'].hot.throttled.totalMs)} de la réf.)`, verdict(rl?.totalMs <= 1428)],
  ['Appels hljs sur bloc figé, ajouts indépendants', 'tous les ticks', '0', `${independent?.highlightCallsAfterFreeze}`, verdict(independent?.highlightCallsAfterFreeze === 0)],
  ['Rendus de blocs figés, ajouts indépendants', 'tous', '0', `${independent?.frozenReplacedTotal} objets remplacés ; ${ms(independent?.renderedBlocksPerAddition)} rendu/ajout`, verdict(independent?.frozenReplacedTotal === 0)],
  ['Rendu final incrémental = complet (corpus)', '—', 'identique', Object.values(now).every((f) => f.finalEqualsReference) ? 'identique (Node, 1 découpage random(1))' : 'DIFFÉRENT', verdict(Object.values(now).every((f) => f.finalEqualsReference))],
  ['KaTeX / Mermaid chargés pour un message ordinaire', '—', 'jamais', 'non mesuré ici : voir le rapport (bundle, chargement dynamique)', 'à vérifier'],
]
for (const row of rows) lines.push(`| ${row.join(' | ')} |`)
lines.push('')
lines.push(
  'Rendu final incrémental : vérifié aussi sur 11 fixtures × 5 découpages dans Chromium (tests/bench/browser), voir le rapport.',
)
lines.push('')
writeFileSync(join(RESULTS, 'after.md'), `${lines.join('\n')}\n`)
console.log(lines.join('\n'))
