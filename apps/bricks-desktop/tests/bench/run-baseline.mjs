#!/usr/bin/env node
// Sprint 14.5 reference measurement of the CURRENT Markdown pipeline (frozen copy in tests/bench/baseline).
// Run from apps/bricks-desktop:  node --expose-gc tests/bench/run-baseline.mjs
// Node + happy-dom (simulated DOM), NOT the WebView. See tests/bench/README.md.

import { execFileSync } from 'node:child_process'
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import os from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { FIXTURE_NAMES, deltaSplits, loadFixture } from '../fixtures/markdown/index.ts'

const SELF = fileURLToPath(import.meta.url)
const BENCH = dirname(SELF)
const ROOT = join(BENCH, '..', '..')
const RESULTS = join(BENCH, 'results')

// Throttle of AssistantMessage.svelte: throttledValue(..., 80).
const TICK_MS = 80
// Simulated arrival rate: 60 UTF-16 units every 80 ms (1.33 ms per unit).
const CHARS_PER_TICK = 60
const MS_PER_UNIT = TICK_MS / CHARS_PER_TICK
// Hot passes; one extra cold pass runs first in a fresh process per fixture.
const ITERATIONS = Math.max(1, Number(process.env.BENCH_ITER ?? 5))
// Worst case "one render per delta": evenly spaced samples when there are more deltas.
const PER_DELTA_SAMPLES = Number(process.env.BENCH_PERDELTA_SAMPLES ?? 40)
// Token-like deltas (1 to 12 code points), fixed seed, so every run streams the same chunks.
const STREAM_SPLIT = 'random(1)'

const [, , flag, fixtureArg] = process.argv
if (flag === '--worker') {
  const result = await measureFixture(fixtureArg)
  process.stdout.write(`${JSON.stringify(result)}\n`)
} else {
  await main()
}

// ---------------------------------------------------------------- orchestration

async function main() {
  const meta = machineMeta()
  const fixtures = []
  for (const name of FIXTURE_NAMES) {
    process.stderr.write(`bench ${name} ...\n`)
    const stdout = execFileSync(process.execPath, ['--expose-gc', SELF, '--worker', name], {
      cwd: ROOT,
      encoding: 'utf8',
      maxBuffer: 64 * 1024 * 1024,
      stdio: ['ignore', 'pipe', 'inherit'],
    })
    fixtures.push(JSON.parse(stdout))
  }
  mkdirSync(RESULTS, { recursive: true })
  writeFileSync(join(RESULTS, `baseline-${meta.date}.json`), `${JSON.stringify({ meta, fixtures }, null, 2)}\n`)
  writeFileSync(join(RESULTS, 'baseline.md'), renderSummary(meta, fixtures))
  process.stderr.write(`done: results/baseline-${meta.date}.json and results/baseline.md\n`)
}

function git(args) {
  try {
    return execFileSync('git', args, { cwd: ROOT, encoding: 'utf8' }).trim()
  } catch {
    return 'inconnu'
  }
}

function pkgVersion(name) {
  return JSON.parse(readFileSync(join(ROOT, 'node_modules', name, 'package.json'), 'utf8')).version
}

function machineMeta() {
  const now = new Date()
  return {
    date: now.toISOString().slice(0, 10),
    startedAt: now.toISOString(),
    cpu: execFileSync('sysctl', ['-n', 'machdep.cpu.brand_string'], { encoding: 'utf8' }).trim(),
    cpuCount: os.cpus().length,
    memoryGB: Math.round(os.totalmem() / 2 ** 30),
    os: `${os.type()} ${os.release()} ${os.arch()}`,
    node: process.version,
    git: `${git(['rev-parse', '--abbrev-ref', 'HEAD'])} @ ${git(['rev-parse', '--short', 'HEAD'])}`,
    versions: {
      'happy-dom': pkgVersion('happy-dom'),
      marked: pkgVersion('marked'),
      dompurify: pkgVersion('dompurify'),
      'highlight.js': pkgVersion('highlight.js'),
    },
    domRuntime: 'happy-dom (DOM simule dans Node, pas la WebView)',
    throttle: { tickMs: TICK_MS, charsPerTick: CHARS_PER_TICK, streamSplit: STREAM_SPLIT },
    iterations: { cold: 1, hot: ITERATIONS, perDeltaSamplesMax: PER_DELTA_SAMPLES },
    gc: 'node --expose-gc (un gc() avant chaque mesure de tas)',
    command: 'cd apps/bricks-desktop && node --expose-gc tests/bench/run-baseline.mjs',
  }
}

// ---------------------------------------------------------------- one fixture (fresh process)

async function measureFixture(name) {
  // The DOM must exist before DOMPurify is imported: it binds to the global window at load time.
  const { Window } = await import('happy-dom')
  globalThis.window = new Window({ url: 'https://bricks.local/' })
  globalThis.document = globalThis.window.document

  const { renderMarkdown } = await import('./baseline/markdown-baseline.ts')
  const hljs = (await import('highlight.js/lib/common')).default
  const hl = instrumentHighlight(hljs)

  const fixture = loadFixture(name)
  const deltas = deltaSplits(fixture.text, STREAM_SPLIT)
  if (deltas.join('') !== fixture.text) throw new Error(`${name}: deltas do not rebuild the text`)
  const deltaEnds = []
  let cumulative = 0
  for (const delta of deltas) {
    cumulative += delta.length
    deltaEnds.push(cumulative)
  }
  const throttledEnds = throttledRenderEnds(deltas.map((delta) => delta.length))
  const sampled = sampleIndices(deltas.length, PER_DELTA_SAMPLES)

  const passes = []
  for (let pass = 0; pass <= ITERATIONS; pass += 1) {
    gc()
    const oneShotStart = performance.now()
    const fullHtml = renderMarkdown(fixture.text)
    const oneShotMs = performance.now() - oneShotStart
    await yieldToEventLoop()

    // Throttled streaming: one render per throttle event, on the simulated clock.
    gc()
    const heapBefore = heapMB()
    let heapPeak = heapBefore
    hl.take()
    const tickMs = []
    let outChars = 0
    for (const end of throttledEnds) {
      const start = performance.now()
      const html = renderMarkdown(fixture.text.slice(0, end))
      tickMs.push(performance.now() - start)
      outChars += html.length
      await yieldToEventLoop()
      heapPeak = Math.max(heapPeak, heapMB())
    }
    const throttledHl = hl.take()
    gc()
    const heapAfter = heapMB()

    // Worst case: one render per delta (evenly spaced samples when there are too many deltas).
    hl.take()
    const perDeltaTick = []
    for (const index of sampled) {
      const start = performance.now()
      renderMarkdown(fixture.text.slice(0, deltaEnds[index]))
      perDeltaTick.push(performance.now() - start)
      await yieldToEventLoop()
    }
    const perDeltaHl = hl.take()

    passes.push({
      oneShotMs,
      outFinalChars: fullHtml.length,
      html: pass === 0 ? fullHtml : undefined,
      throttled: {
        renders: throttledEnds.length,
        tick: tickMs,
        totalMs: sum(tickMs),
        hljsMs: throttledHl.ms,
        hljsCalls: throttledHl.calls,
        outChars,
        heapBeforeMB: heapBefore,
        heapAfterMB: heapAfter,
        heapDeltaMB: heapAfter - heapBefore,
        heapPeakMB: heapPeak,
      },
      perDelta: {
        deltas: deltas.length,
        sampled: sampled.length,
        tick: perDeltaTick,
        meanMs: mean(perDeltaTick),
        hljsMs: perDeltaHl.ms,
      },
    })
  }

  const cold = passes[0]
  const security = probe(cold.html ?? '')
  return {
    name,
    bytes: fixture.bytes,
    lines: fixture.lines,
    deltas: deltas.length,
    throttledRenders: throttledEnds.length,
    security,
    cold: summarize([cold]),
    hot: summarize(passes.slice(1)),
  }
}

/** Renders at which the throttled value is re-rendered, as code-unit prefixes. Mirrors throttledValue (leading + trailing edge). */
function throttledRenderEnds(lengths) {
  const ends = []
  let arrived = 0
  let last = -Infinity
  let timer = null
  const flush = (time) => {
    ends.push(arrived)
    last = time
    timer = null
  }
  for (const length of lengths) {
    const arrival = (arrived + length) * MS_PER_UNIT
    // A pending trailing flush due before this delta runs first, with the text received so far.
    while (timer !== null && timer <= arrival) flush(timer)
    arrived += length
    const wait = TICK_MS - (arrival - last)
    if (wait <= 0) {
      flush(arrival)
    } else if (timer === null) {
      timer = last + TICK_MS
    }
  }
  // End of stream: the pending timer is cleared and the final text is rendered once.
  timer = null
  ends.push(arrived)
  return ends
}

function sampleIndices(count, max) {
  if (count <= max) return Array.from({ length: count }, (_, i) => i)
  return Array.from({ length: max }, (_, k) => Math.floor((k * count) / max))
}

/** Wraps hljs.highlight of the shared module instance: the baseline calls it through the object. */
function instrumentHighlight(hljs) {
  const original = hljs.highlight
  const acc = { ms: 0, calls: 0 }
  hljs.highlight = function instrumented(...args) {
    const start = performance.now()
    try {
      return original.apply(this, args)
    } finally {
      acc.ms += performance.now() - start
      acc.calls += 1
    }
  }
  return {
    take() {
      const out = { ms: acc.ms, calls: acc.calls }
      acc.ms = 0
      acc.calls = 0
      return out
    },
  }
}

function summarize(list) {
  const throttledTicks = list.flatMap((p) => p.throttled.tick).sort((a, b) => a - b)
  const perDeltaTicks = list.flatMap((p) => p.perDelta.tick).sort((a, b) => a - b)
  const throttledTotals = list.map((p) => p.throttled.totalMs)
  const perDeltaTotals = list.map((p) => p.perDelta.meanMs * p.perDelta.deltas)
  const last = list[list.length - 1]
  return {
    passes: list.length,
    oneShotMs: median(list.map((p) => p.oneShotMs)),
    outputCharsFinal: last.outFinalChars,
    throttled: {
      renders: last.throttled.renders,
      totalMs: median(throttledTotals),
      tickP50Ms: percentile(throttledTicks, 0.5),
      tickP95Ms: percentile(throttledTicks, 0.95),
      tickMaxMs: throttledTicks.at(-1) ?? NaN,
      hljsShare: ratio(sum(list.map((p) => p.throttled.hljsMs)), sum(throttledTotals)),
      hljsCallsPerPass: median(list.map((p) => p.throttled.hljsCalls)),
      outputCharsSumPerPass: median(list.map((p) => p.throttled.outChars)),
      heapBeforeMB: last.throttled.heapBeforeMB,
      heapAfterMB: last.throttled.heapAfterMB,
      heapDeltaMB: last.throttled.heapDeltaMB,
      heapPeakMB: last.throttled.heapPeakMB,
    },
    perDelta: {
      deltas: last.perDelta.deltas,
      sampledRenders: last.perDelta.sampled,
      estimatedTotalMs: median(perDeltaTotals),
      tickP50Ms: percentile(perDeltaTicks, 0.5),
      tickP95Ms: percentile(perDeltaTicks, 0.95),
      hljsShare: ratio(
        sum(list.map((p) => p.perDelta.hljsMs)),
        sum(list.map((p) => sum(p.perDelta.tick))),
      ),
    },
  }
}

/** Security probe on the final output, read back through the DOM (nothing is executed or loaded). */
function probe(html) {
  const root = globalThis.document.createElement('div')
  root.innerHTML = html
  const elements = [...root.querySelectorAll('*')]
  const attributes = elements.flatMap((el) =>
    [...el.attributes].map((attr) => ({ tag: el.tagName.toLowerCase(), name: attr.name.toLowerCase(), value: attr.value })),
  )
  const tags = new Set(elements.map((el) => el.tagName.toLowerCase()))
  const dangerous = ['script', 'img', 'svg', 'foreignobject', 'iframe', 'form', 'input', 'object', 'embed', 'style', 'link', 'meta']
  return {
    dangerousTagsPresent: dangerous.filter((tag) => tags.has(tag)),
    eventHandlerAttributes: attributes.filter((a) => a.name.startsWith('on')).length,
    unsafeUriAttributes: attributes.filter(
      (a) => ['href', 'src', 'xlink:href', 'action', 'formaction'].includes(a.name) && /^\s*(javascript|data|vbscript):/i.test(a.value),
    ).length,
    anchors: elements.filter((el) => el.tagName === 'A').length,
  }
}

// ---------------------------------------------------------------- helpers

function gc() {
  if (globalThis.gc) globalThis.gc()
}

/**
 * happy-dom 20.14.5 keeps parsed nodes alive until the event loop turns (pending internal
 * work). Yielding after each render, outside the timed section, lets them be collected.
 */
function yieldToEventLoop() {
  return new Promise((resolve) => setImmediate(resolve))
}

function heapMB() {
  return process.memoryUsage().heapUsed / 2 ** 20
}

function sum(values) {
  return values.reduce((a, b) => a + b, 0)
}

function mean(values) {
  return values.length === 0 ? NaN : sum(values) / values.length
}

function ratio(a, b) {
  return b === 0 ? NaN : a / b
}

function median(values) {
  if (values.length === 0) return NaN
  const sorted = [...values].sort((a, b) => a - b)
  const mid = Math.floor(sorted.length / 2)
  return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2
}

/** Nearest-rank percentile of an ascending array. */
function percentile(sorted, p) {
  if (sorted.length === 0) return NaN
  const index = Math.min(sorted.length - 1, Math.max(0, Math.ceil(p * sorted.length) - 1))
  return sorted[index]
}

// ---------------------------------------------------------------- Markdown summary

function renderSummary(meta, fixtures) {
  const ms = (x) => (Number.isFinite(x) ? x.toFixed(x < 10 ? 2 : 1) : 'n/a')
  const pct = (x) => (Number.isFinite(x) ? `${(x * 100).toFixed(0)} %` : 'n/a')
  const rows = fixtures.map((f) => {
    const c = f.cold
    const h = f.hot
    return (
      `| ${f.name} | ${f.bytes} | ${f.deltas} | ${h.throttled.renders} | ` +
      `${ms(c.oneShotMs)} / ${ms(h.oneShotMs)} | ` +
      `${ms(c.throttled.totalMs)} / ${ms(h.throttled.totalMs)} | ` +
      `${ms(h.throttled.tickP50Ms)} / ${ms(h.throttled.tickP95Ms)} | ` +
      `${pct(h.throttled.hljsShare)} | ` +
      `${ms(h.perDelta.estimatedTotalMs)}${h.perDelta.sampledRenders < h.perDelta.deltas ? ' (estimé)' : ''} | ` +
      `${ms(h.perDelta.tickP50Ms)} / ${ms(h.perDelta.tickP95Ms)} | ` +
      `${h.throttled.heapDeltaMB.toFixed(1)} |`
    )
  })
  return [
    '# Référence Markdown : pipeline actuel (commit c1fe3de)',
    '',
    `Mesure du ${meta.date} : ${meta.cpu}, ${meta.cpuCount} cœurs, ${meta.memoryGB} Go, ${meta.os}, ${meta.node}.`,
    `Node + happy-dom ${meta.versions['happy-dom']} (DOM simulé, pas la WebView). marked ${meta.versions.marked}, DOMPurify ${meta.versions.dompurify}, highlight.js ${meta.versions['highlight.js']}.`,
    `Streaming simulé : throttle ${TICK_MS} ms, ${CHARS_PER_TICK} car./${TICK_MS} ms, deltas \`${STREAM_SPLIT}\`. ` +
      `Froid = 1re passe dans un processus neuf ; chaud = médiane de ${ITERATIONS} passes.`,
    '',
    '| Fixture | Octets | Deltas | Rendus (throttle) | Plein rendu froid / chaud (ms) | Throttle total froid / chaud (ms) | Tick p50 / p95 chaud (ms) | Part hljs | Un rendu par delta, total (ms) | Par delta p50 / p95 (ms) | Δ tas (MB) |',
    '| --- | --: | --: | --: | --: | --: | --: | --: | --: | --: | --: |',
    ...rows,
    '',
    'Limites : pas de WebView ni de WKWebView ; DOM happy-dom (plus lent ou plus rapide que le moteur réel selon les nœuds) ;',
    'la cadence du throttle est simulée (le temps de rendu ne retarde pas le tick suivant) ;',
    '"un rendu par delta" est échantillonné (40 points répartis) quand il y a plus de 40 deltas, puis extrapolé ; ',
    'Δ tas = différence de heapUsed après gc (indicatif) ; la part hljs inclut le coût de l\'instrumentation.',
    '',
  ].join('\n')
}
