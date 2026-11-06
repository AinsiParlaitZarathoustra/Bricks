<script lang="ts">
  import { t } from '$lib/i18n/i18n.svelte'
  import Code from '@lucide/svelte/icons/code'
  import Copy from '@lucide/svelte/icons/copy'
  import Download from '@lucide/svelte/icons/download'
  import FileImage from '@lucide/svelte/icons/file-image'
  import ImageDown from '@lucide/svelte/icons/image-down'
  import Maximize2 from '@lucide/svelte/icons/maximize-2'
  import Move from '@lucide/svelte/icons/move'
  import ZoomIn from '@lucide/svelte/icons/zoom-in'
  import ZoomOut from '@lucide/svelte/icons/zoom-out'
  import * as Dialog from '$lib/components/ui/dialog'
  import { app } from '$lib/state/app.svelte'
  import { theme } from '$lib/theme/theme.svelte'
  import { diagramMode, diagramSettings, diagramTheme } from '$lib/features/diagrams/settings'
  import { savePngFile, saveSvgFile } from '$lib/ipc/client'
  import { exportableSvg } from './mermaid'
  import { svgFrame } from './svg-viewer'
  import { effectiveBackground, svgToPng } from './svg-png'

  /**
   * `mermaid`: a diagram in the chosen Mermaid theme; `chart`: a chart drawn in
   * the app's colours; `drawing`: an SVG the model wrote; `file`: a file read.
   */
  type Variant = 'mermaid' | 'chart' | 'drawing' | 'file'

  let { svg, source, title = t('conversation.diagram'), fileName = t('conversation.diagramSvg'), variant = source !== undefined ? 'mermaid' : 'file' }: {
    svg: string; source?: string; title?: string; fileName?: string; variant?: Variant
  } = $props()
  const settings = $derived(diagramSettings(app.prefs.diagrams))
  const options = $derived(settings.diagramViewer)
  // A diagram or chart drawn in the app's colours sits on the app's code background;
  // any other SVG (a Mermaid theme, a drawing, a file) keeps a plain background of its own.
  const appColours = $derived(variant === 'chart' || (variant === 'mermaid' && diagramTheme(settings) === 'app'))
  const mode = $derived(
    variant === 'chart' ? theme.resolved : variant === 'mermaid' ? diagramMode(settings, theme.resolved) : 'light',
  )
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
  let showSource = $state(false)
  let figure = $state<HTMLElement | null>(null)
  /** WebKit and Chromium write images to the clipboard; elsewhere the button is not shown. */
  const canCopyImage = typeof ClipboardItem !== 'undefined' && typeof navigator !== 'undefined' && !!navigator.clipboard?.write
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
    try { await navigator.clipboard.writeText(source ?? frame.svg); flash(t('conversation.sourceCopied')) }
    catch { flash(t('common.couldNotCopy')) }
  }
  async function download() {
    if (!options.allowDownload || !frame.svg) return
    try { if (await saveSvgFile(fileName, exportableSvg(frame.svg))) flash(t('conversation.svgSaved')) }
    catch { flash(t('conversation.couldNotExport')) }
  }
  /** The colour the diagram is seen on: the frame's own plain background, else the figure's. */
  function imageBackground(): string {
    if (!appColours) return mode === 'dark' ? '#111827' : '#ffffff'
    return effectiveBackground(figure, mode === 'dark' ? '#111827' : '#ffffff')
  }
  /** The whole diagram as a PNG, whatever the zoom or the position of the view. */
  function png(): Promise<Blob> {
    return svgToPng(exportableSvg(frame.svg), frame.size, imageBackground())
  }
  function copyImage() {
    if (!options.allowCopy || !frame.svg || !canCopyImage) return
    // Written at once, inside the click: WebKit keeps the user's gesture only
    // until the first await; the image itself comes later, as a promise.
    const image = png()
    let written: Promise<void>
    try {
      written = navigator.clipboard.write([new ClipboardItem({ 'image/png': image })])
    } catch {
      image.catch(() => {})
      flash(t('common.couldNotCopy'))
      return
    }
    Promise.all([written, image]).then(
      () => flash(t('conversation.imageCopied')),
      () => flash(t('common.couldNotCopy')),
    )
  }
  /** What `save_png_file` accepts at most (32 Mio). */
  const PNG_LIMIT = 32 * 1024 * 1024
  async function downloadPng() {
    if (!options.allowDownload || !frame.svg) return
    try {
      const blob = await png()
      // The backend refuses more; the bytes are not even sent.
      if (blob.size > PNG_LIMIT) {
        flash(t('conversation.pngTooLarge'))
        return
      }
      const name = fileName.replace(/\.svg$/i, '') + '.png'
      if (await savePngFile(name, new Uint8Array(await blob.arrayBuffer()))) flash(t('conversation.pngSaved'))
    } catch { flash(t('conversation.couldNotExport')) }
  }
  const action = 'inline-flex size-7 items-center justify-center rounded text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-40'
</script>

{#snippet controls(fullscreen = false)}
  <div class="flex flex-wrap items-center gap-0.5">
    {#if options.allowZoom && frame.svg}
      <button type="button" class={action} title={t('conversation.zoomOut')} aria-label={t('conversation.zoomOut')} disabled={zoom <= 0.5} onclick={() => (zoom = Math.max(0.5, zoom - 0.25))}><ZoomOut class="size-3.5" aria-hidden="true" /></button>
      <button type="button" class="rounded px-1 text-xs tabular-nums" title={t('conversation.resetTheView')} aria-label={t('conversation.resetTheView')} onclick={reset}>{Math.round(zoom * 100)} %</button>
      <button type="button" class={action} title={t('conversation.zoomIn')} aria-label={t('conversation.zoomIn')} disabled={zoom >= 4} onclick={() => (zoom = Math.min(4, zoom + 0.25))}><ZoomIn class="size-3.5" aria-hidden="true" /></button>
    {/if}
    {#if options.allowPan && frame.svg}
      <button type="button" class={action} title={t('conversation.moveTheDiagramDragOrUse')} aria-label={t('conversation.moveTheDiagram')} aria-pressed={moving} onclick={() => (moving = !moving)}><Move class="size-3.5" aria-hidden="true" /></button>
    {/if}
    {#if options.allowFullscreen && frame.svg && !fullscreen}
      <button type="button" class={action} title={t('common.fullscreen')} aria-label={t('common.fullscreen')} onclick={() => (expanded = true)}><Maximize2 class="size-3.5" aria-hidden="true" /></button>
    {/if}
    {#if options.allowDownload && frame.svg}
      <button type="button" class={action} title={t('conversation.exportAsSvg')} aria-label={t('conversation.exportAsSvg')} onclick={download}><Download class="size-3.5" aria-hidden="true" /></button>
    {/if}
    {#if options.allowDownload && frame.svg}
      <button type="button" class={action} title={t('conversation.exportAsPng')} aria-label={t('conversation.exportAsPng')} onclick={downloadPng}><FileImage class="size-3.5" aria-hidden="true" /></button>
    {/if}
    {#if source !== undefined}
      <button type="button" class={action} title={t('conversation.showTheSource')} aria-label={t('conversation.showTheSource')} aria-pressed={showSource} onclick={() => (showSource = !showSource)}><Code class="size-3.5" aria-hidden="true" /></button>
    {/if}
    {#if options.allowCopy}
      <button type="button" class={action} title={t('conversation.copyTheSource')} aria-label={t('conversation.copyTheSource')} onclick={copy}><Copy class="size-3.5" aria-hidden="true" /></button>
      {#if canCopyImage}
        <button type="button" class={action} title={t('conversation.copyImage')} aria-label={t('conversation.copyImage')} disabled={!frame.svg} onclick={copyImage}><ImageDown class="size-3.5" aria-hidden="true" /></button>
      {/if}
    {/if}
    {#if feedback}<span role="status" class="px-1 text-xs">{feedback}</span>{/if}
  </div>
{/snippet}

{#snippet viewport(fullscreen = false)}
  <div class={['relative overflow-hidden', !fullscreen && 'mx-auto w-full']} style={fullscreen ? `height: calc(100dvh - ${showSource && source !== undefined ? '25rem' : '8rem'});` : inlineSize}>
    <div class="size-full origin-center" style:transform>
      <iframe title={title} sandbox="" referrerpolicy="no-referrer" srcdoc={frame.document}
        class="size-full border-0" style:pointer-events={pan ? 'none' : 'auto'}></iframe>
    </div>
    {#if pan}
      <button type="button" aria-label={t('conversation.moveTheDiagramDragOrUse2')}
        class="absolute inset-0 size-full cursor-grab touch-none outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
        onpointerdown={start} onpointermove={drag} onpointerup={stop} onpointercancel={stop} onlostpointercapture={stop} onkeydown={key}>
        <span class="sr-only">{t('conversation.moveTheDiagram')}</span>
      </button>
    {/if}
  </div>
{/snippet}

{#snippet sourceView()}
  {#if showSource && source !== undefined}
    <pre class="max-h-80 overflow-auto border-t border-border bg-code-bg p-2.5 text-xs leading-relaxed text-code-foreground"><code class="font-mono">{source}</code></pre>
  {/if}
{/snippet}

<figure bind:this={figure} class={['my-2 overflow-hidden rounded-md border border-border text-card-foreground', appColours ? 'bg-code-bg' : 'bg-card']}>
  <figcaption class="flex flex-wrap items-center justify-between gap-1 px-2 py-1 text-xs text-muted-foreground">
    <span>{title}</span>{@render controls()}
  </figcaption>
  {#if frame.svg}
    {#if !(expanded && options.allowFullscreen)}{@render viewport()}{/if}
  {:else}
    <p role="alert" class="p-3 text-xs text-destructive">{t('conversation.invalidOrTooLargeSvg')}</p>
  {/if}
  {#if !expanded}{@render sourceView()}{/if}
</figure>
<Dialog.Root open={expanded && options.allowFullscreen} onOpenChange={(open) => (expanded = open)}>
  <Dialog.Content class="flex h-[100dvh] w-screen max-w-none flex-col gap-2 rounded-none sm:max-w-none">
    <Dialog.Header><Dialog.Title class="text-center text-ui">{title}</Dialog.Title></Dialog.Header>
    {@render controls(true)}
    {#if frame.svg}{@render viewport(true)}{/if}
    {@render sourceView()}
  </Dialog.Content>
</Dialog.Root>
