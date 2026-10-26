<script lang="ts">
  // A top-level ```mermaid fence of an assistant message. The renderer is
  // loaded on demand, only once the fence is closed; the source stays visible
  // until then and when rendering fails. The SVG comes from `renderMermaid`,
  // already validated by `sanitizeSvg`.
  import { fontsReady, theme } from '$lib/theme/theme.svelte'
  import { app } from '$lib/state/app.svelte'
  import { diagramSettings, diagramTheme } from '$lib/features/diagrams/settings'
  import { renderMermaid, type MermaidCache } from './mermaid'
  import { mermaidColors } from './mermaid-colors'
  import SvgViewer from './SvgViewer.svelte'

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
  let instance = 0

  const settings = $derived(diagramSettings(app.prefs.diagrams))
  const renderTheme = $derived(diagramTheme(settings))
  const renderLook = $derived(settings.mermaid.look)
  const securityLevel = $derived(settings.mermaid.securityLevel)

  // Render only a closed fence, once per source and theme. A late result for
  // an older source or theme is dropped.
  $effect(() => {
    const text = source
    const chosen = renderTheme
    const mode = theme.resolved
    const look = renderLook
    const level = securityLevel
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
        theme: chosen,
        mode,
        look,
        securityLevel: level,
        // Read after the mode's class is set: the app's colours of that mode.
        colors: chosen === 'app' ? mermaidColors() : undefined,
        fontFamily: getComputedStyle(document.documentElement).getPropertyValue('--ui-font-family').trim() || 'sans-serif',
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

</script>

{#if svg}
  <SvgViewer {svg} {source} title="Mermaid" />
{:else}
  <figure class="my-2 overflow-hidden rounded-md bg-code-bg text-code-foreground" aria-busy={!error && complete}>
    {#if error}<p class="px-2.5 pt-2 text-xs text-destructive" role="alert">Diagramme non rendu : {error}</p>{/if}
    <pre class="overflow-x-auto p-2.5 text-xs leading-relaxed"><code class="font-mono">{source}</code></pre>
  </figure>
{/if}
