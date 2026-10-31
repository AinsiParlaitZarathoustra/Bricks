<script lang="ts">
  // The run at work: « Travaille depuis 1m 29s », counted from the engine's start
  // timestamp of the run (approval waits included, as in the « bilan »). Without a
  // known start, no duration is shown. The timer lives only while this text is mounted;
  // the work block (WorkBlock.svelte) places it in its bar.
  import type { Conversation } from '$lib/state/conversation.svelte'
  import { createClock } from '$lib/features/navigation/clock.svelte'
  import { formatWorked } from '../runend'

  let { conversation }: { conversation: Conversation } = $props()

  const TICK_MS = 1000

  const startedAt = $derived.by(() => {
    const id = conversation.runningRunId
    return id ? conversation.runs.get(id)?.startedAt : undefined
  })

  const clock = createClock(TICK_MS)

  /** Whole seconds only: the line never shows milliseconds. */
  const elapsed = $derived.by(() => {
    if (startedAt === undefined) return null
    const whole = Math.floor(Math.max(0, clock.now - startedAt) / TICK_MS) * TICK_MS
    return whole < TICK_MS ? '0s' : formatWorked(whole)
  })
</script>

{#if elapsed}
  <span class="tabular-nums">Travaille depuis {elapsed}</span>
{:else}
  <span>Travaille…</span>
{/if}
