<script lang="ts">
  // A top-level ```mermaid fence of an assistant message. The renderer is
  // loaded on demand, only once the fence is closed; the source stays visible
  // until then and when rendering fails. The SVG comes from `renderMermaid`,
  // already validated by `sanitizeSvg`.
  import Copy from '@lucide/svelte/icons/copy'
  import Download from '@lucide/svelte/icons/download'
  import Maximize2 from '@lucide/svelte/icons/maximize-2'
  import ZoomIn from '@lucide/svelte/icons/zoom-in'
  import ZoomOut from '@lucide/svelte/icons/zoom-out'
  import * as Dialog from '$lib/components/ui/dialog'
  import { saveSvgFile } from '$lib/ipc/client'
  import { fontsReady, theme } from '$lib/theme/theme.svelte'
  import { exportableSvg, renderMermaid, type MermaidCache } from './mermaid'
  import { mermaidColors } from './mermaid-colors'

  interface Props {
    source: string
    complete: boolean
    /** Unique per message and block: the SVG ids derive from it. */
    id: string
    cache: MermaidCache
  }

  let { source, complete, id, cache }: Props = $props()

  let svg = $state<string | null>(null)
  let error = $state<string | null>(null)
  let zoom = $state(1)
  let expanded = $state(false)
  let feedback = $state<string | null>(null)
  let instance = 0

  // Render only a closed fence, once per source and theme. A late result for
  // an older source or theme is dropped.
  $effect(() => {
    const text = source
    const mode = theme.resolved
    const version = ++instance
    // A replacement/open fence must never keep the previous diagram exportable.
    svg = null
    error = null
    if (!complete) return
    let cancelled = false
    void (async () => {
      await fontsReady
      if (cancelled) return
      const result = await renderMermaid(text, {
        id: `${id}-${version}`,
        theme: mode,
        fontFamily: getComputedStyle(document.documentElement).getPropertyValue('--font-sans').trim() || 'sans-serif',
        colors: mermaidColors(),
        cache,
      })
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

  let feedbackTimer: ReturnType<typeof setTimeout> | undefined
  function flash(text: string) {
    feedback = text
    clearTimeout(feedbackTimer)
    feedbackTimer = setTimeout(() => (feedback = null), 1500)
  }
  $effect(() => () => clearTimeout(feedbackTimer))

  async function copySource() {
    try {
      await navigator.clipboard.writeText(source)
      flash('Source copiée')
    } catch {
      flash('Copie impossible')
    }
  }

  async function exportSvg() {
    if (!svg) return
    try {
      const path = await saveSvgFile('diagramme.svg', exportableSvg(svg))
      if (path) flash('SVG enregistré')
    } catch (e) {
      flash(e instanceof Error ? e.message : 'Export impossible')
    }
  }

  const action =
    'inline-flex size-6 items-center justify-center rounded text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none disabled:opacity-40'
</script>

<figure class="mermaid-block my-2 overflow-hidden rounded-md bg-code-bg text-code-foreground" aria-busy={!svg && !error}>
  <div class="flex items-center justify-between gap-2 px-2.5 py-1 text-xs text-muted-foreground">
    <span class="font-mono">mermaid</span>
    <div class="flex items-center gap-0.5">
      {#if feedback}<span role="status" class="mr-1">{feedback}</span>{/if}
      {#if svg}
        <button type="button" class={action} title="Zoom arrière" aria-label="Zoom arrière" disabled={zoom <= 0.5} onclick={() => (zoom = Math.max(0.5, zoom - 0.25))}>
          <ZoomOut class="size-3.5" aria-hidden="true" />
        </button>
        <button type="button" class="rounded px-1 tabular-nums hover:bg-muted hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none" title="Réinitialiser le zoom" onclick={() => (zoom = 1)}>
          {Math.round(zoom * 100)} %
        </button>
        <button type="button" class={action} title="Zoom avant" aria-label="Zoom avant" disabled={zoom >= 3} onclick={() => (zoom = Math.min(3, zoom + 0.25))}>
          <ZoomIn class="size-3.5" aria-hidden="true" />
        </button>
        <button type="button" class={action} title="Agrandir" aria-label="Agrandir" onclick={() => (expanded = true)}>
          <Maximize2 class="size-3.5" aria-hidden="true" />
        </button>
        <button type="button" class={action} title="Exporter en SVG" aria-label="Exporter en SVG" onclick={exportSvg}>
          <Download class="size-3.5" aria-hidden="true" />
        </button>
      {/if}
      <button type="button" class={action} title="Copier la source" aria-label="Copier la source" onclick={copySource}>
        <Copy class="size-3.5" aria-hidden="true" />
      </button>
    </div>
  </div>

  {#if svg}
    <div class="overflow-auto px-2.5 pb-2.5">
      <div class="mermaid-svg" style:zoom>
        {#if !expanded}{@html svg}{/if}
      </div>
    </div>
  {:else}
    {#if error}
      <p class="px-2.5 text-xs text-destructive" role="alert">Diagramme non rendu : {error}</p>
    {/if}
    <pre class="overflow-x-auto p-2.5 text-xs leading-relaxed"><code class="font-mono">{source}</code></pre>
  {/if}
</figure>

<Dialog.Root bind:open={expanded}>
  <Dialog.Content class="flex max-h-[90vh] w-[95vw] flex-col gap-3 sm:max-w-[95vw]">
    <Dialog.Header>
      <Dialog.Title class="text-[13px]">Diagramme</Dialog.Title>
    </Dialog.Header>
    {#if svg}
      <div class="mermaid-svg mermaid-expanded min-h-0 flex-1 overflow-auto">
        {@html svg}
      </div>
    {/if}
  </Dialog.Content>
</Dialog.Root>

<style>
  /* Mermaid sets width 100% and its natural width as max-width: the diagram
     fills the message column up to its own size; the zoom scales the wrapper. */
  .mermaid-svg :global(svg) {
    display: block;
    margin-inline: auto;
    height: auto;
    max-height: 26rem;
  }
  /* Mermaid's inline max-width is its natural size: the enlarged view lifts it. */
  .mermaid-expanded :global(svg) {
    width: 100%;
    max-width: 100% !important;
    max-height: 78vh;
  }
</style>
