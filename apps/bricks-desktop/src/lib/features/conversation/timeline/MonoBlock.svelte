<script lang="ts">
  import { cn } from '$lib/utils.js'

  interface Props {
    text: string
    /** Accessible name of the scroll region, e.g. « Sortie de Bash ». */
    label: string
    /** Colours `+` and `-` lines as a unified diff. */
    diff?: boolean
    /** Lines shown before the « Afficher tout » control. */
    maxLines?: number
    class?: string
  }

  let { text, label, diff = false, maxLines = 200, class: className }: Props = $props()

  let showAll = $state(false)
  const lines = $derived(text.split('\n'))
  const shown = $derived(showAll ? lines : lines.slice(0, maxLines))
  const hiddenCount = $derived(lines.length - shown.length)

  function lineClass(line: string): string {
    if (!diff) return ''
    if (line.startsWith('+++') || line.startsWith('---')) return 'text-muted-foreground'
    if (line.startsWith('+')) return 'bg-diff-add-bg text-diff-add'
    if (line.startsWith('-')) return 'bg-diff-del-bg text-diff-del'
    if (line.startsWith('@@')) return 'text-info'
    return ''
  }
</script>

<!-- The block scrolls, so keyboard users must be able to focus it (WCAG 2.1.1). -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<div
  role="region"
  aria-label={label}
  tabindex="0"
  class={cn(
    'max-h-80 overflow-auto rounded-md border border-border bg-code-bg text-code-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none',
    className,
  )}
>
  <pre class="min-w-max p-2.5 font-mono text-xs leading-relaxed">{#each shown as line, i (i)}<span class={cn('block whitespace-pre', lineClass(line))}>{line || ' '}</span>{/each}</pre>
</div>
{#if hiddenCount > 0}
  <button
    type="button"
    class="mt-1 rounded text-xs text-primary underline-offset-4 hover:underline focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
    onclick={() => (showAll = true)}
  >
    Afficher tout ({hiddenCount} ligne{hiddenCount > 1 ? 's' : ''} de plus)
  </button>
{/if}
