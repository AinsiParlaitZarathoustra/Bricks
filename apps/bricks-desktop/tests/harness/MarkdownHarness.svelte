<script lang="ts">
  // Streams a corpus fixture into the real AssistantMessage component and
  // measures, in this browser: time of each throttled update (render + DOM
  // flush), long tasks, DOM mutations of blocks already frozen, heap, and
  // whether KaTeX / Mermaid were loaded. Results in `window.__harness`.
  import { flushSync, tick } from 'svelte'
  import { renderMarkdown as renderBaseline } from '../bench/baseline/markdown-baseline'
  import AssistantMessage from '../../src/lib/features/conversation/timeline/AssistantMessage.svelte'
  import type { AssistantItem, Conversation } from '../../src/lib/state/conversation.svelte'

  const sources = import.meta.glob('../fixtures/markdown/*.md', { query: '?raw', import: 'default', eager: true }) as Record<string, string>
  const fixtures = Object.fromEntries(Object.entries(sources).map(([path, text]) => [path.split('/').pop()!.replace('.md', ''), text]))

  const params = new URLSearchParams(location.search)
  let name = $state(params.get('fixture') ?? 'simple')
  const chunk = Number(params.get('chunk') ?? 60)
  // fast=1: a simulated clock steps 85 ms per update (past the 80 ms throttle)
  // and the loop yields through MessageChannel, which the pane does not throttle.
  // Each step is still one real Svelte + DOM render; only waiting is skipped.
  const fast = params.get('fast') === '1'
  // engine=baseline: the frozen pipeline of c1fe3de (whole message re-parsed, one {@html}).
  const baseline = params.get('engine') === 'baseline'
  let baselineHtml = $state('')
  let fakeNow = Date.now()
  if (fast) Date.now = () => fakeNow
  const channel = new MessageChannel()
  const yieldTask = () => new Promise<void>((resolve) => {
    channel.port1.onmessage = () => resolve()
    channel.port2.postMessage(0)
  })

  let item = $state<AssistantItem>({ kind: 'assistant', id: 'h1', text: '', thinking: '', streaming: false })
  const conversation = { projectId: null } as unknown as Conversation
  let status = $state('prêt')
  let root = $state<HTMLDivElement | null>(null)

  // Browser-only controls to verify source replacement and cancellation between
  // throttled updates without a provider, real session or production fixture.
  ;(window as unknown as { __markdownControl: unknown }).__markdownControl = {
    replace(text: string, streaming = false) {
      item.text = text
      item.streaming = streaming
      flushSync()
    },
    stop() {
      item.streaming = false
      flushSync()
    },
  }

  function percentile(values: number[], p: number) {
    if (values.length === 0) return 0
    const sorted = [...values].sort((a, b) => a - b)
    return sorted[Math.min(sorted.length - 1, Math.floor((p / 100) * sorted.length))]
  }

  function loaded(pattern: RegExp) {
    return performance.getEntriesByType('resource').some((e) => pattern.test(e.name))
  }

  async function run() {
    const text = fixtures[name] ?? ''
    status = 'streaming'
    const longTasks: number[] = []
    let observer: PerformanceObserver | null = null
    try {
      observer = new PerformanceObserver((list) => list.getEntries().forEach((e) => longTasks.push(e.duration)))
      observer.observe({ type: 'longtask', buffered: false })
    } catch {
      observer = null
    }
    item = { kind: 'assistant', id: `h${Date.now()}`, text: '', thinking: '', streaming: true }
    await tick()
    const ticks: number[] = []
    // Mutations of block nodes that already existed before the last update.
    let frozenMutations = 0
    const count = (records: MutationRecord[]) => {
      for (const r of records) {
        if (r.type === 'childList') r.removedNodes.forEach((n) => { if (n.nodeType === 1) frozenMutations += 1 })
      }
    }
    const mutations = new MutationObserver(count)
    const md = () => root?.querySelector('.md')
    for (let i = chunk; i < text.length + chunk; i += chunk) {
      const prefix = text.slice(0, Math.min(i, text.length))
      const target = md()
      if (target) mutations.observe(target, { childList: true })
      const t0 = performance.now()
      if (baseline) baselineHtml = renderBaseline(prefix)
      else item.text = prefix
      flushSync()
      ticks.push(performance.now() - t0)
      count(mutations.takeRecords())
      mutations.disconnect()
      // Past the 80 ms throttle: each step is one real render.
      if (fast) {
        fakeNow += 85
        await yieldTask()
      } else {
        await new Promise((r) => setTimeout(r, 85))
      }
    }
    const t0 = performance.now()
    if (baseline) baselineHtml = renderBaseline(text)
    else item.streaming = false
    flushSync()
    const finalMs = performance.now() - t0
    await new Promise((r) => setTimeout(r, fast ? 3000 : 1500))
    observer?.disconnect()
    const memory = (performance as unknown as { memory?: { usedJSHeapSize: number } }).memory
    const result = {
      fixture: name,
      engine: baseline ? 'baseline' : 'blocks',
      chars: text.length,
      updates: ticks.length,
      totalMs: Math.round(ticks.reduce((a, b) => a + b, 0)),
      p50: +percentile(ticks, 50).toFixed(2),
      p95: +percentile(ticks, 95).toFixed(2),
      max: +Math.max(...ticks).toFixed(2),
      finalMs: +finalMs.toFixed(2),
      longTasks: longTasks.length,
      longTaskMs: Math.round(longTasks.reduce((a, b) => a + b, 0)),
      removedElements: frozenMutations,
      heapMB: memory ? +(memory.usedJSHeapSize / 1048576).toFixed(1) : null,
      katexLoaded: loaded(/deps\/katex|katex(\.min)?\.(css|js|mjs)/i),
      mermaidLoaded: loaded(/deps\/mermaid|mermaid\.core|node_modules\/mermaid/i),
      katexNodes: document.querySelectorAll('.katex').length,
      mermaidSvgs: document.querySelectorAll('.mermaid-block svg').length,
      mathErrors: document.querySelectorAll('.math-error').length,
    }
    ;(window as unknown as { __harness: unknown }).__harness = result
    status = 'fini'
  }
</script>

<div class="mx-auto flex max-w-3xl flex-col gap-3 p-4">
  <div class="flex items-center gap-2 text-xs">
    <select bind:value={name} class="rounded border px-1">
      {#each Object.keys(fixtures) as f (f)}<option value={f}>{f}</option>{/each}
    </select>
    <button type="button" class="rounded border px-2" onclick={run}>Lancer</button>
    <span data-status>{status}</span>
  </div>
  <div bind:this={root} data-conversation-scroll>
    {#if baseline}
      <div class="md text-sm leading-relaxed">{@html baselineHtml}</div>
    {:else}
      <AssistantMessage {item} {conversation} />
    {/if}
  </div>
</div>
