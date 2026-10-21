<script lang="ts">
	// The context occupation of the shown conversation, compact: its share and one
	// bar, from the same figures as the composer's ring. Unknown or stale reads `—`.
	import type { Conversation } from '$lib/state/conversation.svelte';
	import SectionCard from '$lib/layout/SectionCard.svelte';
	import { contextGauge, formatCompact, formatPercent } from './contextView';

	interface Props {
		conversation: Conversation;
	}

	let { conversation }: Props = $props();

	const gauge = $derived(contextGauge(conversation.context ?? null, conversation.contextStale ?? false));
	const share = $derived(gauge.percent === null ? '—' : formatPercent(gauge.percent));
	const figures = $derived(
		`${gauge.used === null ? '—' : formatCompact(gauge.used)} / ${gauge.total === null ? '—' : formatCompact(gauge.total)}`
	);
</script>

<SectionCard title="Contexte">
	<div class="flex items-baseline justify-between gap-3 text-xs">
		<span class="font-medium tabular-nums text-foreground">{share}</span>
		<span class="tabular-nums text-muted-foreground">{figures}</span>
	</div>
	<div
		class="mt-2 h-1.5 overflow-hidden rounded-full bg-muted"
		role="meter"
		aria-label="Contexte utilisé"
		aria-valuemin={0}
		aria-valuemax={100}
		aria-valuenow={gauge.percent === null ? undefined : Math.round(Math.min(100, gauge.percent))}
		aria-valuetext={gauge.percent === null ? 'inconnu' : `${share} (${figures})`}
	>
		{#if gauge.ratio !== null}
			<div
				class={['h-full rounded-full', gauge.ratio >= 0.9 ? 'bg-destructive' : gauge.ratio >= 0.75 ? 'bg-warning' : 'bg-primary']}
				style:width={`${gauge.ratio * 100}%`}
			></div>
		{/if}
	</div>
</SectionCard>
