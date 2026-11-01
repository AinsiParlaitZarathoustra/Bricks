<script lang="ts">
  // A top-level ```vega-lite fence of an assistant message: the chart once the
  // fence is closed, drawn as an SVG at the width available, in the app's theme
  // under the chart's own colours. Vega is loaded on demand. While it streams,
  // and when it cannot be drawn, its source stays shown.
  import { onMount } from 'svelte'
  import { fontsReady, theme } from '$lib/theme/theme.svelte'
  import { mermaidColors } from './mermaid-colors'
  import { renderVegaLite } from './vega'
  import SvgViewer from './SvgViewer.svelte'

  let { source, complete }: { source: string; complete: boolean } = $props()

  let svg = $state<string | null>(null)
  let error = $state<string | null>(null)
  let width = $state(0)
  let host = $state<HTMLElement | null>(null)
  let instance = 0

  // The width available, followed (after a pause) while the panel is resized.
  onMount(() => {
    if (!host) return
    let timer: ReturnType<typeof setTimeout> | undefined
    const observer = new ResizeObserver((entries) => {
      const next = Math.round(entries[0]?.contentRect.width ?? 0)
      clearTimeout(timer)
      timer = setTimeout(() => {
        if (Math.abs(next - width) >= 8) width = next
      }, width === 0 ? 0 : 200)
    })
    observer.observe(host)
    return () => {
      clearTimeout(timer)
      observer.disconnect()
    }
  })

  // Drawn again for a new source, width or mode; a late result is dropped.
  $effect(() => {
    const text = source
    const available = width
    void theme.resolved
    const version = ++instance
    // The previous chart stays shown while the next one is drawn (a resize).
    if (!complete || available === 0) {
      if (!complete) {
        svg = null
        error = null
      }
      return
    }
    let cancelled = false
    void (async () => {
      // The theme's fonts, when they do not load, are no reason to draw nothing.
      await fontsReady.catch(() => {})
      if (cancelled) return
      let result: Awaited<ReturnType<typeof renderVegaLite>>
      try {
        result = await renderVegaLite(text, {
          // The viewer's own padding aside.
          width: available - 24,
          // Read after the mode's class is set: the app's colours of that mode.
          colors: mermaidColors(),
          fontFamily: getComputedStyle(document.documentElement).getPropertyValue('--ui-font-family').trim() || 'sans-serif',
        })
      } catch (e) {
        result = { error: e instanceof Error ? e.message : String(e) }
      }
      if (cancelled || version !== instance) return
      if ('svg' in result) {
        svg = result.svg
        error = null
      } else {
        svg = null
        error = result.error
      }
    })()
    return () => {
      cancelled = true
    }
  })
</script>

<div bind:this={host} class="w-full">
  {#if svg}
    <SvgViewer {svg} {source} title="Vega-Lite" variant="chart" fileName="graphique.svg" />
  {:else}
    <figure class="my-2 overflow-hidden rounded-md bg-code-bg text-code-foreground" aria-busy={!error && complete}>
      {#if error}<p class="px-2.5 pt-2 text-xs text-destructive" role="alert">Graphique non rendu : {error}</p>{/if}
      <pre class="overflow-x-auto p-2.5 text-xs leading-relaxed"><code class="font-mono">{source}</code></pre>
    </figure>
  {/if}
</div>
