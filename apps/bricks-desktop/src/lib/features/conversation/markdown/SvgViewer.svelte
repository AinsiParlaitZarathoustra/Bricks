<script lang="ts">
  import Copy from '@lucide/svelte/icons/copy'
  import Download from '@lucide/svelte/icons/download'
  import Maximize2 from '@lucide/svelte/icons/maximize-2'
  import Move from '@lucide/svelte/icons/move'
  import ZoomIn from '@lucide/svelte/icons/zoom-in'
  import ZoomOut from '@lucide/svelte/icons/zoom-out'
  import * as Dialog from '$lib/components/ui/dialog'
  import { app } from '$lib/state/app.svelte'
  import { theme } from '$lib/theme/theme.svelte'
  import { diagramMode, diagramSettings, diagramTheme } from '$lib/features/diagrams/settings'
  import { saveSvgFile } from '$lib/ipc/client'
  import { exportableSvg } from './mermaid'
  import { svgFrame } from './svg-viewer'

  let { svg, source, title = 'Diagramme', fileName = 'diagramme.svg' }: {
    svg: string; source?: string; title?: string; fileName?: string
  } = $props()
  const settings = $derived(diagramSettings(app.prefs.diagrams))
  const options = $derived(settings.diagramViewer)
  // A Mermaid diagram drawn in the app's colours sits on the app's code background;
  // any other SVG (a Mermaid theme, a file) keeps a plain background of its own.
  const appColours = $derived(source !== undefined && diagramTheme(settings) === 'app')
  const mode = $derived(source !== undefined ? diagramMode(settings, theme.resolved) : 'light')
  const frame = $derived(svgFrame(svg, options, mode, appColours ? 'transparent' : 'plain'))
  /** At most 26rem high, and never wider than the drawing itself. */
  const inlineSize = $derived(
    frame.size
      ? `aspect-ratio: ${frame.size.width} / ${frame.size.height}; max-width: ${frame.size.width}px; max-height: 26rem;`
      : 'height: 26rem;',
  )
  let zoom = $state(1)
  let x = $state(0)
  let y = $state(0)
  let moving = $state(false)
  let expanded = $state(false)
  let feedback = $state<string | null>(null)
  let pointer: { id: number; x: number; y: number; startX: number; startY: number } | null = null
  let timer: ReturnType<typeof setTimeout> | undefined
  $effect(() => () => clearTimeout(timer))
  const pan = $derived(options.allowPan && moving)
  const transform = $derived(`translate(${options.allowPan ? x : 0}px, ${options.allowPan ? y : 0}px) scale(${options.allowZoom ? zoom : 1})`)

  function flash(text: string) {
    feedback = text
    clearTimeout(timer)
    timer = setTimeout(() => (feedback = null), 1800)
  }
  function reset() { zoom = 1; x = 0; y = 0 }
  function start(event: PointerEvent) {
    if (!pan || event.button !== 0) return
    const target = event.currentTarget as HTMLElement
    target.setPointerCapture(event.pointerId)
    pointer = { id: event.pointerId, x: event.clientX, y: event.clientY, startX: x, startY: y }
  }
  function drag(event: PointerEvent) {
    if (!pan || pointer?.id !== event.pointerId) return
    x = pointer.startX + event.clientX - pointer.x
    y = pointer.startY + event.clientY - pointer.y
  }
  function stop() { pointer = null }
  function key(event: KeyboardEvent) {
    if (!pan) return
    const shifts: Record<string, [number, number]> = { ArrowLeft: [-24, 0], ArrowRight: [24, 0], ArrowUp: [0, -24], ArrowDown: [0, 24] }
    const shift = shifts[event.key]
    if (!shift) return
    event.preventDefault(); x += shift[0]; y += shift[1]
  }
  async function copy() {
    if (!options.allowCopy) return
    try { await navigator.clipboard.writeText(source ?? frame.svg); flash('Source copiée') }
    catch { flash('Copie impossible') }
  }
  async function download() {
    if (!options.allowDownload || !frame.svg) return
    try { if (await saveSvgFile(fileName, exportableSvg(frame.svg))) flash('SVG enregistré') }
    catch { flash('Export impossible') }
  }
  const action = 'inline-flex size-7 items-center justify-center rounded text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-40'
</script>

{#snippet controls(fullscreen = false)}
  <div class="flex flex-wrap items-center gap-0.5">
    {#if options.allowZoom && frame.svg}
      <button type="button" class={action} title="Zoom arrière" aria-label="Zoom arrière" disabled={zoom <= 0.5} onclick={() => (zoom = Math.max(0.5, zoom - 0.25))}><ZoomOut class="size-3.5" aria-hidden="true" /></button>
      <button type="button" class="rounded px-1 text-xs tabular-nums" title="Réinitialiser la vue" aria-label="Réinitialiser la vue" onclick={reset}>{Math.round(zoom * 100)} %</button>
      <button type="button" class={action} title="Zoom avant" aria-label="Zoom avant" disabled={zoom >= 4} onclick={() => (zoom = Math.min(4, zoom + 0.25))}><ZoomIn class="size-3.5" aria-hidden="true" /></button>
    {/if}
    {#if options.allowPan && frame.svg}
      <button type="button" class={action} title="Déplacer le diagramme (glisser ou utiliser les flèches)" aria-label="Déplacer le diagramme" aria-pressed={moving} onclick={() => (moving = !moving)}><Move class="size-3.5" aria-hidden="true" /></button>
    {/if}
    {#if options.allowFullscreen && frame.svg && !fullscreen}
      <button type="button" class={action} title="Plein écran" aria-label="Plein écran" onclick={() => (expanded = true)}><Maximize2 class="size-3.5" aria-hidden="true" /></button>
    {/if}
    {#if options.allowDownload && frame.svg}
      <button type="button" class={action} title="Exporter en SVG" aria-label="Exporter en SVG" onclick={download}><Download class="size-3.5" aria-hidden="true" /></button>
    {/if}
    {#if options.allowCopy}
      <button type="button" class={action} title="Copier la source" aria-label="Copier la source" onclick={copy}><Copy class="size-3.5" aria-hidden="true" /></button>
    {/if}
    {#if feedback}<span role="status" class="px-1 text-xs">{feedback}</span>{/if}
  </div>
{/snippet}

{#snippet viewport(fullscreen = false)}
  <div class={['relative overflow-hidden', !fullscreen && 'mx-auto w-full']} style={fullscreen ? 'height: calc(100dvh - 8rem);' : inlineSize}>
    <div class="size-full origin-center" style:transform>
      <iframe title={title} sandbox="" referrerpolicy="no-referrer" srcdoc={frame.document}
        class="size-full border-0" style:pointer-events={pan ? 'none' : 'auto'}></iframe>
    </div>
    {#if pan}
      <button type="button" aria-label="Déplacer le diagramme : glisser ou utiliser les flèches"
        class="absolute inset-0 size-full cursor-grab touch-none outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
        onpointerdown={start} onpointermove={drag} onpointerup={stop} onpointercancel={stop} onlostpointercapture={stop} onkeydown={key}>
        <span class="sr-only">Déplacer le diagramme</span>
      </button>
    {/if}
  </div>
{/snippet}

<figure class={['my-2 overflow-hidden rounded-md border border-border text-card-foreground', appColours ? 'bg-code-bg' : 'bg-card']}>
  <figcaption class="flex flex-wrap items-center justify-between gap-1 px-2 py-1 text-xs text-muted-foreground">
    <span>{title}</span>{@render controls()}
  </figcaption>
  {#if frame.svg}
    {#if !(expanded && options.allowFullscreen)}{@render viewport()}{/if}
  {:else}
    <p role="alert" class="p-3 text-xs text-destructive">SVG invalide ou trop volumineux.</p>
  {/if}
</figure>
<Dialog.Root open={expanded && options.allowFullscreen} onOpenChange={(open) => (expanded = open)}>
  <Dialog.Content class="flex h-[100dvh] w-screen max-w-none flex-col gap-2 rounded-none sm:max-w-none">
    <Dialog.Header><Dialog.Title class="text-[13px]">{title}</Dialog.Title></Dialog.Header>
    {@render controls(true)}
    {#if frame.svg}{@render viewport(true)}{/if}
  </Dialog.Content>
</Dialog.Root>
