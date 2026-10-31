<script lang="ts">
	// The context occupation of the shown conversation, compact: its share, one bar
	// and, once a request was sent on this context, its sources. The bar's fill is
	// the occupation; each source colours the part of it its share of the last
	// request's text stands for. Unknown or stale reads `—`.
	import type { Conversation } from '$lib/state/conversation.svelte';
	import SectionCard from '$lib/layout/SectionCard.svelte';
	import { contextGauge, contextShares, formatPercent, formatShort, type ContextSource } from './contextView';

	interface Props {
		conversation: Conversation;
	}

	let { conversation }: Props = $props();

	const status = $derived(conversation.context ?? null);
	const stale = $derived(conversation.contextStale ?? false);
	const gauge = $derived(contextGauge(status, stale));
	const shares = $derived(contextShares(status, stale));
	const share = $derived(gauge.percent === null ? '—' : `${Math.round(gauge.percent)} %`);
	const figures = $derived(
		`${gauge.used === null ? '—' : formatShort(gauge.used)} / ${gauge.total === null ? '—' : formatShort(gauge.total)}`
	);

	/** What the input budget still has room for, once a breakdown is shown. */
	const free = $derived(
		shares && gauge.used !== null && gauge.total !== null ? Math.max(0, gauge.total - gauge.used) : null
	);

	// Bricks' blue for the messages, then vivid hues of its palette; lighter in the dark theme.
	const COLOR: Record<ContextSource, string> = {
		conversation: 'bg-primary dark:bg-[oklch(0.62_0.22_264)]',
		tool_results: 'bg-[oklch(0.65_0.19_42)] dark:bg-[oklch(0.73_0.17_42)]',
		tool_definitions: 'bg-[oklch(0.63_0.16_160)] dark:bg-[oklch(0.76_0.15_160)]',
		instructions: 'bg-[oklch(0.75_0.16_80)] dark:bg-[oklch(0.83_0.15_85)]',
		other: 'bg-chart-5'
	};
</script>

<SectionCard title="Contexte">
	<div class="flex items-baseline justify-between gap-3 text-ui">
		<span
			class={[
				'font-medium tabular-nums',
				gauge.ratio !== null && gauge.ratio >= 0.9
					? 'text-destructive'
					: gauge.ratio !== null && gauge.ratio >= 0.75
						? 'text-warning'
						: 'text-foreground'
			]}>{share} utilisé</span
		>
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
			{#if shares}
				<div class="flex h-full gap-px" style:width={`${gauge.ratio * 100}%`}>
					{#each shares as part (part.source)}
						<div class={['h-full first:rounded-l-full last:rounded-r-full', COLOR[part.source]]} style:flex-grow={part.share}></div>
					{/each}
				</div>
			{:else}
				<div
					class={['h-full rounded-full', gauge.ratio >= 0.9 ? 'bg-destructive' : gauge.ratio >= 0.75 ? 'bg-warning' : 'bg-primary']}
					style:width={`${gauge.ratio * 100}%`}
				></div>
			{/if}
		{/if}
	</div>
	{#if shares}
		<ul class="mt-2.5 flex flex-col gap-1 text-xs" aria-label="Répartition du contexte">
			{#each shares as part (part.source)}
				<li class="flex min-w-0 items-center gap-1.5">
					<span aria-hidden="true" class={['size-2 shrink-0 rounded-sm', COLOR[part.source]]}></span>
					<span class="min-w-0 flex-1 truncate text-foreground" title={part.label}>{part.label}</span>
					<span class="shrink-0 tabular-nums text-muted-foreground">{formatShort(part.tokens)} tokens</span>
					<span class="w-11 shrink-0 text-right whitespace-nowrap tabular-nums text-foreground">{formatPercent(part.share * 100)}</span>
				</li>
			{/each}
			{#if free !== null}
				<li class="flex min-w-0 items-center gap-1.5">
					<span aria-hidden="true" class="size-2 shrink-0 rounded-sm border border-border bg-muted"></span>
					<span class="min-w-0 flex-1 truncate text-muted-foreground">Espace libre</span>
					<span class="shrink-0 tabular-nums text-muted-foreground">{formatShort(free)} tokens</span>
					<span class="w-11 shrink-0" aria-hidden="true"></span>
				</li>
			{/if}
		</ul>
	{/if}
</SectionCard>
