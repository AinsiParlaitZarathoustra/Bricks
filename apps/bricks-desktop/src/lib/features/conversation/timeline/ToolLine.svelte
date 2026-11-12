<script lang="ts">
  // One light line for a tool call: no card, no arguments, no output. Used by the
  // timeline and by the sub-agent inspector. The icon is the tool's; the state is
  // said only when the call did not complete (« Échec », « Interrompu »).
  import type { Snippet } from 'svelte'
  import { TOOL_LINE_LABEL, formatToolDuration, type ToolLineState } from './toolLine'
  import { toolIcon } from './toolIcons'
  import { cn } from '$lib/utils.js'

  interface Props {
    /** Real tool name, as the engine reported it. */
    name: string
    state: ToolLineState
    durationMs?: number
    children?: Snippet
  }

  let { name, state, durationMs, children }: Props = $props()

  const Icon = $derived(toolIcon(name))
  /** Shown once the call has finished, and only when the engine gave a duration. */
  const duration = $derived(state === 'running' ? null : formatToolDuration(durationMs))
</script>

<div class="flex min-w-0 items-center gap-2 py-0.5 text-xs text-muted-foreground">
  <Icon
    class={cn('size-3.5 shrink-0', state === 'failed' && 'text-destructive')}
    aria-hidden="true"
  />
  <span class={cn('min-w-0 truncate font-mono text-foreground', state === 'running' && 'shimmer motion-reduce:shimmer-none')}>{name}</span>
  {@render children?.()}
  {#if state === 'failed'}
    <span class="shrink-0 text-destructive">{TOOL_LINE_LABEL.failed}</span>
  {:else if state === 'interrupted'}
    <span class="shrink-0">{TOOL_LINE_LABEL.interrupted}</span>
  {:else}
    <span class="sr-only">{TOOL_LINE_LABEL[state]}</span>
  {/if}
  {#if duration}
    <span class="ml-auto shrink-0 whitespace-nowrap tabular-nums">{duration}</span>
  {/if}
</div>
