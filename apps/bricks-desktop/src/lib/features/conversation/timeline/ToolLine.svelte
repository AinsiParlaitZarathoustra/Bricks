<script lang="ts">
  // One light line for a tool call: no card, no arguments, no output. Used by the
  // timeline and by the sub-agent inspector.
  import CircleCheck from '@lucide/svelte/icons/circle-check'
  import CircleStop from '@lucide/svelte/icons/circle-stop'
  import CircleX from '@lucide/svelte/icons/circle-x'
  import LoaderCircle from '@lucide/svelte/icons/loader-circle'
  import { TOOL_LINE_LABEL, formatToolDuration, type ToolLineState } from './toolLine'

  interface Props {
    /** Real tool name, as the engine reported it. */
    name: string
    state: ToolLineState
    durationMs?: number
  }

  let { name, state, durationMs }: Props = $props()

  /** Shown once the call has finished, and only when the engine gave a duration. */
  const duration = $derived(state === 'running' ? null : formatToolDuration(durationMs))
</script>

<div class="flex min-w-0 items-center gap-2 py-0.5 text-xs text-muted-foreground">
  {#if state === 'running'}
    <LoaderCircle class="size-3.5 shrink-0 animate-spin text-info" aria-hidden="true" />
  {:else if state === 'done'}
    <CircleCheck class="size-3.5 shrink-0 text-success" aria-hidden="true" />
  {:else if state === 'failed'}
    <CircleX class="size-3.5 shrink-0 text-destructive" aria-hidden="true" />
  {:else}
    <CircleStop class="size-3.5 shrink-0 text-warning" aria-hidden="true" />
  {/if}
  <span class="min-w-0 truncate font-mono text-foreground">{name}</span>
  <span class="shrink-0">{TOOL_LINE_LABEL[state]}</span>
  {#if duration}
    <span class="ml-auto shrink-0 whitespace-nowrap tabular-nums">{duration}</span>
  {/if}
</div>
