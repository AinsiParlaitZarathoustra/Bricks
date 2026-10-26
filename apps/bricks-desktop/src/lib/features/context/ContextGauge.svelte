<script lang="ts">
  // The composer's context ring, left of the model select: occupation of the
  // next request over the model window (the same figures as the context card).
  // Hover or focus shows a small panel. Unknown or stale: a neutral ring.
  import * as Tooltip from '$lib/components/ui/tooltip'
  import type { Conversation } from '$lib/state/conversation.svelte'
  import { cacheHitRate, contextGauge, formatPercent, gaugeLine } from './contextView'

  let { conversation }: { conversation: Conversation | null } = $props()

  const gauge = $derived(contextGauge(conversation?.context ?? null, conversation?.contextStale ?? false))
  const cache = $derived(cacheHitRate(conversation?.context ?? null))
  const value = $derived(gaugeLine(gauge))

  // Circle of radius 6.5 in a 16×16 box drawn at 15 px, 2.2 stroke: the size of the
  // reference ring next to the model select. The fill starts at noon, clockwise.
  const R = 6.5
  const CIRCUMFERENCE = 2 * Math.PI * R
  const dash = $derived(gauge.ratio === null ? 0 : gauge.ratio * CIRCUMFERENCE)
  const label = $derived(gauge.percent === null ? 'Contexte : inconnu' : `Contexte : ${value}`)
</script>

<Tooltip.Root delayDuration={150}>
  <Tooltip.Trigger
    class="inline-flex size-6 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
    aria-label={label}
  >
    <svg viewBox="0 0 16 16" class="size-[15px] -rotate-90" aria-hidden="true">
      <circle cx="8" cy="8" r={R} fill="none" stroke-width="2.2" class="gauge-track" />
      {#if dash > 0}
        <circle
          cx="8"
          cy="8"
          r={R}
          fill="none"
          stroke-width="2.2"
          stroke-dasharray="{dash} {CIRCUMFERENCE}"
          class="gauge-fill"
        />
      {/if}
    </svg>
  </Tooltip.Trigger>
  <Tooltip.Content
    side="top"
    sideOffset={8}
    arrowClasses="hidden"
    class="flex w-72 max-w-none flex-col items-stretch gap-2.5 rounded-xl border bg-popover px-3.5 py-3 text-popover-foreground shadow-md"
  >
    <div class="flex items-baseline justify-between gap-3">
      <span class="text-ui font-medium">Contexte</span>
      <span class="font-mono text-xs text-muted-foreground tabular-nums">{value}</span>
    </div>
    <div class="h-1.5 overflow-hidden rounded-full bg-muted" aria-hidden="true">
      {#if gauge.ratio !== null}
        <div class="h-full rounded-full bg-foreground" style:width="{gauge.ratio * 100}%"></div>
      {/if}
    </div>
    <div class="flex items-baseline justify-between gap-3 text-xs">
      <span class="text-muted-foreground">Taux de cache moyen</span>
      <span class="font-mono tabular-nums">{cache === null ? '—' : formatPercent(cache)}</span>
    </div>
  </Tooltip.Content>
</Tooltip.Root>

<style>
  .gauge-track {
    stroke: color-mix(in oklab, var(--muted-foreground) 38%, transparent);
  }
  .gauge-fill {
    stroke: var(--muted-foreground);
  }
</style>
