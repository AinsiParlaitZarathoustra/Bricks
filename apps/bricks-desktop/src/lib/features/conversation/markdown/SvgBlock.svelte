<script lang="ts">
  // A top-level ```svg fence of an assistant message: the drawing once the fence
  // is closed, in the same viewer as the other diagrams (sanitized, sandboxed).
  // While it streams, and when it is invalid or too large, its source stays shown.
  import { t } from '$lib/i18n/i18n.svelte'
  import { sanitizeSvg } from './sanitize'
  import { MAX_SVG_LENGTH } from './svg-viewer'
  import SvgViewer from './SvgViewer.svelte'

  let { source, complete }: { source: string; complete: boolean } = $props()

  const error = $derived.by(() => {
    if (!complete) return null
    if (source.length > MAX_SVG_LENGTH) return t('conversation.svgTooLarge5MibAt')
    // A drawing, not a page that holds one somewhere.
    if (!/^\s*(?:<\?xml[^>]*\?>\s*)?(?:<!--[\s\S]*?-->\s*)*(?:<!DOCTYPE[^>]*>\s*)?(?:<!--[\s\S]*?-->\s*)*<svg[\s>]/i.test(source)) {
      return t('conversation.theRootMustBeAnSvg')
    }
    return sanitizeSvg(source) ? null : t('conversation.invalidSvg')
  })
</script>

{#if complete && !error}
  <SvgViewer svg={source} {source} title="SVG" variant="drawing" fileName={t('conversation.drawingSvg')} />
{:else}
  <figure class="my-2 overflow-hidden rounded-md bg-code-bg text-code-foreground" aria-busy={!complete}>
    {#if error}<p class="px-2.5 pt-2 text-xs text-destructive" role="alert">{t('conversation.drawingNotRendered', { error })}</p>{/if}
    <pre class="overflow-x-auto p-2.5 text-xs leading-relaxed"><code class="font-mono">{source}</code></pre>
  </figure>
{/if}
