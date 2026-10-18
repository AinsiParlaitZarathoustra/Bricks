#!/usr/bin/env node
// Sprint 14.5 measurement of the NEW pipeline: one MarkdownDocument per message, streamed like
// AssistantMessage.svelte (throttle 80 ms, 60 units per 80 ms, same deltas as run-baseline.mjs).
// Run from apps/bricks-desktop:  node --expose-gc tests/bench/run-after.mjs
// Node + happy-dom (simulated DOM), NOT the WebView. Same harness as run-baseline.mjs; see tests/bench/README.md.

import { execFileSync } from 'node:child_process'
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import os from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { registerHooks } from 'node:module'
import { FIXTURE_NAMES, deltaSplits, loadFixture } from '../fixtures/markdown/index.ts'

// The new modules import their siblings without extension (Vite style); Node's type stripping needs
// the exact file, so `.ts` is tried when the specifier does not resolve.
registerHooks({
  resolve(specifier, context, nextResolve) {
    try {
      return nextResolve(specifier, context)
    } catch (error) {
      if (error?.code === 'ERR_MODULE_NOT_FOUND' && /^\.{1,2}\//.test(specifier) && !/\.[cm]?[jt]s$/.test(specifier)) {
        return nextResolve(`${specifier}.ts`, context)
      }
      throw error
    }
  },
})

const SELF = fileURLToPath(import.meta.url)
const BENCH = dirname(SELF)
const ROOT = join(BENCH, '..', '..')
const RESULTS = join(BENCH, 'results')

const TICK_MS = 80
const CHARS_PER_TICK = 60
const MS_PER_UNIT = TICK_MS / CHARS_PER_TICK
const ITERATIONS = Math.max(1, Number(process.env.BENCH_ITER ?? 5))
const STREAM_SPLIT = 'random(1)'
const OPTIONS = { idPrefix: 'm1-bench' }
/** Synthetic message for the independent-additions check: one code block, then paragraphs appended one by one. */
const INDEPENDENT = 'independent-additions'

const [, , flag, fixtureArg] = process.argv
if (flag === '--worker') {
  const result = fixtureArg === INDEPENDENT ? await measureIndependent() : await measureFixture(fixtureArg)
  process.stdout.write(`${JSON.stringify(result)}\n`)
} else {
  await main()
}

// ---------------------------------------------------------------- orchestration

async function main() {
  const meta = machineMeta()
  const fixtures = []
  for (const name of [...FIXTURE_NAMES, INDEPENDENT]) {
    process.stderr.write(`bench ${name} ...\n`)
    const stdout = execFileSync(process.execPath, ['--expose-gc', SELF, '--worker', name], {
      cwd: ROOT,
      encoding: 'utf8',
      maxBuffer: 256 * 1024 * 1024,
      stdio: ['ignore', 'pipe', 'inherit'],
    })
    fixtures.push(JSON.parse(stdout))
  }
  mkdirSync(RESULTS, { recursive: true })
  writeFileSync(join(RESULTS, `after-${meta.date}.json`), `${JSON.stringify({ meta, fixtures }, null, 2)}\n`)
  process.stderr.write(`done: results/after-${meta.date}.json (summary written by tests/bench/summarize-after.mjs)\n`)
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
    git: `${git(['rev-parse', '--abbrev-ref', 'HEAD'])} @ ${git(['rev-parse', '--short', 'HEAD'])} (+ working tree)`,
    versions: {
      'happy-dom': pkgVersion('happy-dom'),
      marked: pkgVersion('marked'),
      dompurify: pkgVersion('dompurify'),
      'highlight.js': pkgVersion('highlight.js'),
    },
    domRuntime: 'happy-dom (DOM simule dans Node, pas la WebView)',
    throttle: { tickMs: TICK_MS, charsPerTick: CHARS_PER_TICK, streamSplit: STREAM_SPLIT },
    iterations: { cold: 1, hot: ITERATIONS },
    option: OPTIONS,
    command: 'cd apps/bricks-desktop && node --expose-gc tests/bench/run-after.mjs',
  }
}

// ---------------------------------------------------------------- DOM and modules of one process

async function setup() {
  // The DOM must exist before DOMPurify is imported: it binds to the global window at load time.
  const { Window } = await import('happy-dom')
  globalThis.window = new Window({ url: 'https://bricks.local/' })
  globalThis.document = globalThis.window.document
  const doc = await import('../../src/lib/features/conversation/markdown/document.ts')
  const hljs = (await import('highlight.js/lib/common')).default
  return { ...doc, hl: instrumentHighlight(hljs) }
}

// ---------------------------------------------------------------- one fixture (fresh process)

async function measureFixture(name) {
  const { MarkdownDocument, renderReference, hl } = await setup()
  const fixture = loadFixture(name)
  const text = fixture.text
  const deltas = deltaSplits(text, STREAM_SPLIT)
  if (deltas.join('') !== text) throw new Error(`${name}: deltas do not rebuild the text`)
  const throttledEnds = throttledRenderEnds(deltas.map((delta) => delta.length))

  const referenceJson = JSON.stringify(renderReference(text, OPTIONS))
  let finalEqual = null
  const passes = []
  for (let pass = 0; pass <= ITERATIONS; pass += 1) {
    gc()
    const oneShotStart = performance.now()
    const reference = renderReference(text, OPTIONS)
    const oneShotMs = performance.now() - oneShotStart
    await yieldToEventLoop()
    if (pass === 0) finalEqual = JSON.stringify(reference) === referenceJson

    const streamed = await streamMessage(MarkdownDocument, text, throttledEnds, hl)
    if (pass === 0) finalEqual = finalEqual && streamed.finalJson === referenceJson

    // Cold pass only: every delta as one update, no throttle (worst case without throttling, incremental).
    let perDelta = null
    if (pass === 0) {
      const doc = new MarkdownDocument(OPTIONS)
      hl.take()
      const tick = []
      let prefix = ''
      for (const delta of deltas) {
        prefix += delta
        const start = performance.now()
        doc.update(prefix, false)
        tick.push(performance.now() - start)
        await yieldToEventLoop()
      }
      doc.dispose()
      const highlight = hl.take()
      perDelta = {
        deltas: deltas.length,
        tickP50Ms: percentile([...tick].sort((a, b) => a - b), 0.5),
        tickP95Ms: percentile([...tick].sort((a, b) => a - b), 0.95),
        totalMs: sum(tick),
        hljsCalls: highlight.calls,
      }
    }
    passes.push({ oneShotMs, ...streamed, perDelta })
  }

  const cold = passes[0]
  return {
    name,
    bytes: fixture.bytes,
    lines: fixture.lines,
    deltas: deltas.length,
    throttledRenders: throttledEnds.length,
    finalEqualsReference: finalEqual,
    cold: summarize([cold]),
    hot: summarize(passes.slice(1)),
  }
}

/**
 * One message streamed like AssistantMessage.svelte: one MarkdownDocument, an update per throttle
 * tick with the text received so far (streaming), then the end of stream as update(text, true).
 */
async function streamMessage(MarkdownDocument, text, throttledEnds, hl) {
  gc()
  const heapBefore = heapMB()
  let heapPeak = heapBefore
  const doc = new MarkdownDocument(OPTIONS)
  hl.take()
  const tickMs = []
  const ticks = []
  let outChars = 0
  let previous = new Map()
  let previousLength = 0
  let frozenReplaced = 0
  let frozenChecked = 0
  let repeatedFrozenHighlight = 0
  let finalJson = ''
  for (let i = 0; i < throttledEnds.length; i += 1) {
    const end = throttledEnds[i]
    const last = i === throttledEnds.length - 1
    const source = text.slice(0, end)
    const start = performance.now()
    const blocks = doc.update(source, last)
    tickMs.push(performance.now() - start)
    const stats = doc.stats
    ticks.push({
      renderedBlocks: stats.renderedBlocks,
      reusedBlocks: stats.reusedBlocks,
      highlightCalls: stats.highlightCalls,
      lexedChars: stats.lexedChars,
      fullLex: stats.fullLex,
      fallback: stats.fallback === true,
      invalidatedBlocks: stats.invalidatedBlocks,
    })
    outChars += blocks.reduce((n, b) => n + (b.kind === 'html' ? b.html.length : 0), 0)
    // A frozen block is every block but the last two of the previous output: its object must survive.
    const current = new Map(blocks.map((b) => [b.key, b]))
    const frozenKeys = previousLength > 2 ? [...previous.keys()].slice(0, previousLength - 2) : []
    if (stats.invalidatedBlocks === 0) {
      for (const key of frozenKeys) {
        frozenChecked += 1
        const after = current.get(key)
        if (after !== undefined && after !== previous.get(key)) frozenReplaced += 1
      }
    }
    previous = new Map(blocks.map((b) => [b.key, b]))
    previousLength = blocks.length
    await yieldToEventLoop()
    heapPeak = Math.max(heapPeak, heapMB())
    if (last) finalJson = JSON.stringify(blocks)
  }
  const highlight = hl.take()
  gc()
  const heapAfter = heapMB()
  void repeatedFrozenHighlight
  return {
    tickMs,
    totalMs: sum(tickMs),
    outChars,
    ticks,
    highlightWrapperCalls: highlight.calls,
    highlightWrapperMs: highlight.ms,
    frozenChecked,
    frozenReplaced,
    heapBeforeMB: heapBefore,
    heapAfterMB: heapAfter,
    heapDeltaMB: heapAfter - heapBefore,
    heapPeakMB: heapPeak,
    finalJson,
  }
}

// ---------------------------------------------------------------- independent additions (synthetic)

/**
 * A rust code block, then paragraphs appended one at a time: once the block is frozen, no
 * highlight call and no re-render may concern it (the check of THRESHOLDS.md).
 */
async function measureIndependent() {
  const { MarkdownDocument, hl } = await setup()
  const code = loadFixture('rust-long').text.split('\n').slice(0, 60).join('\n')
  const head = `Texte d'introduction.\n\n\`\`\`rust\n${code}\n\`\`\`\n\n`
  const paragraphs = Array.from({ length: 300 }, (_, k) => `Paragraphe ${k + 1} : une phrase indépendante, sans lien avec le bloc.\n\n`)
  const doc = new MarkdownDocument(OPTIONS)
  hl.take()
  let source = head
  const rows = []
  let previous = new Map()
  let previousLength = 0
  let frozenReplaced = 0
  for (let k = 0; k < paragraphs.length; k += 1) {
    source += paragraphs[k]
    const start = performance.now()
    const blocks = doc.update(source, false)
    const ms = performance.now() - start
    const stats = doc.stats
    const current = new Map(blocks.map((b) => [b.key, b]))
    const frozenKeys = previousLength > 2 ? [...previous.keys()].slice(0, previousLength - 2) : []
    let replaced = 0
    if (stats.invalidatedBlocks === 0) {
      for (const key of frozenKeys) if (current.get(key) !== undefined && current.get(key) !== previous.get(key)) replaced += 1
    }
    frozenReplaced += replaced
    rows.push({ k, ms, highlightCalls: stats.highlightCalls, renderedBlocks: stats.renderedBlocks, replaced })
    previous = current
    previousLength = blocks.length
    await yieldToEventLoop()
  }
  const after = rows.slice(5)
  return {
    name: INDEPENDENT,
    paragraphsAppended: paragraphs.length,
    highlightCallsAfterFreeze: sum(after.map((r) => r.highlightCalls)),
    renderedBlocksPerAddition: mean(after.map((r) => r.renderedBlocks)),
    frozenReplacedTotal: frozenReplaced,
    tickP95Ms: percentile(rows.map((r) => r.ms).sort((a, b) => a - b), 0.95),
    wrapperHighlightCalls: hl.take().calls,
  }
}

// ---------------------------------------------------------------- throttle (copy of run-baseline.mjs)

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
    while (timer !== null && timer <= arrival) flush(timer)
    arrived += length
    const wait = TICK_MS - (arrival - last)
    if (wait <= 0) {
      flush(arrival)
    } else if (timer === null) {
      timer = last + TICK_MS
    }
  }
  timer = null
  ends.push(arrived)
  return ends
}

/** Wraps hljs.highlight of the shared module instance (the renderer calls it through the object). */
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
  const ticks = list.flatMap((p) => p.ticks)
  const tickSorted = list.flatMap((p) => p.tickMs).sort((a, b) => a - b)
  const totals = list.map((p) => p.totalMs)
  const last = list[list.length - 1]
  return {
    passes: list.length,
    oneShotMs: median(list.map((p) => p.oneShotMs)),
    finalEqualsReference: list.every((p) => p.finalJson !== undefined),
    throttled: {
      renders: last.tickMs.length,
      totalMs: median(totals),
      tickP50Ms: percentile(tickSorted, 0.5),
      tickP95Ms: percentile(tickSorted, 0.95),
      tickMaxMs: tickSorted.at(-1) ?? NaN,
      hljsCallsDocStats: median(list.map((p) => sum(p.ticks.map((t) => t.highlightCalls)))),
      hljsCallsWrapper: median(list.map((p) => p.highlightWrapperCalls)),
      hljsShare: ratio(sum(list.map((p) => p.highlightWrapperMs)), sum(totals)),
      renderedBlocksPerPass: median(list.map((p) => sum(p.ticks.map((t) => t.renderedBlocks)))),
      reusedBlocksPerPass: median(list.map((p) => sum(p.ticks.map((t) => t.reusedBlocks)))),
      lexedCharsPerPass: median(list.map((p) => sum(p.ticks.map((t) => t.lexedChars)))),
      fullLexTicks: median(list.map((p) => p.ticks.filter((t) => t.fullLex).length)),
      fallbackTicks: sum(list.map((p) => p.ticks.filter((t) => t.fallback).length)),
      invalidatedBlocksPerPass: median(list.map((p) => sum(p.ticks.map((t) => t.invalidatedBlocks)))),
      frozenChecked: median(list.map((p) => p.frozenChecked)),
      frozenReplacedPerPass: median(list.map((p) => p.frozenReplaced)),
      outputCharsSumPerPass: median(list.map((p) => p.outChars)),
      heapBeforeMB: last.heapBeforeMB,
      heapAfterMB: last.heapAfterMB,
      heapDeltaMB: last.heapDeltaMB,
      heapPeakMB: last.heapPeakMB,
    },
    perDelta: list[0].perDelta,
  }
}

// ---------------------------------------------------------------- helpers

function gc() {
  if (globalThis.gc) globalThis.gc()
}

/** Yields after each update, outside the timed section: happy-dom keeps parsed nodes until the loop turns. */
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
