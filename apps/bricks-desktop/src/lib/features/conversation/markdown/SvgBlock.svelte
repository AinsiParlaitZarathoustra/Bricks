<script lang="ts">
  // A top-level ```svg fence of an assistant message: the drawing once the fence
  // is closed, in the same viewer as the other diagrams (sanitized, sandboxed).
  // While it streams, and when it is invalid or too large, its source stays shown.
  import { sanitizeSvg } from './sanitize'
  import { MAX_SVG_LENGTH } from './svg-viewer'
  import SvgViewer from './SvgViewer.svelte'

  let { source, complete }: { source: string; complete: boolean } = $props()

  const error = $derived.by(() => {
    if (!complete) return null
    if (source.length > MAX_SVG_LENGTH) return 'SVG trop volumineux (5 Mio au maximum)'
    // A drawing, not a page that holds one somewhere.
    if (!/^\s*(?:<\?xml[^>]*\?>\s*)?(?:<!--[\s\S]*?-->\s*)*(?:<!DOCTYPE[^>]*>\s*)?(?:<!--[\s\S]*?-->\s*)*<svg[\s>]/i.test(source)) {
      return 'la racine doit être un élément <svg>'
    }
    return sanitizeSvg(source) ? null : 'SVG invalide'
  })
</script>

{#if complete && !error}
  <SvgViewer svg={source} {source} title="SVG" variant="drawing" fileName="dessin.svg" />
{:else}
  <figure class="my-2 overflow-hidden rounded-md bg-code-bg text-code-foreground" aria-busy={!complete}>
    {#if error}<p class="px-2.5 pt-2 text-xs text-destructive" role="alert">Dessin non rendu : {error}</p>{/if}
    <pre class="overflow-x-auto p-2.5 text-xs leading-relaxed"><code class="font-mono">{source}</code></pre>
  </figure>
{/if}
