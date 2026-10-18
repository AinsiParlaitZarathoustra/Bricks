<script lang="ts">
	import type { Conversation } from '$lib/state/conversation.svelte';
	import SectionCard from '$lib/layout/SectionCard.svelte';
	import { contextLine, formatCost, formatNumber, formatTokens } from './contextView';

	interface Props {
		conversation: Conversation;
	}

	let { conversation }: Props = $props();

	const status = $derived(conversation.context);
	const stale = $derived(conversation.contextStale);
	const line = $derived(contextLine(status, stale));
	const totalCost = $derived(conversation.usageTotal?.costUsd);
	/** A figure the engine reported (0 included), or `—` when there is none yet. */
	const tokens = (n: number | undefined) => (n === undefined ? '—' : formatTokens(n));
	/** Some responses had no price: the amount is a lower bound, said in its accessible name. */
	const partialCost = $derived(conversation.usageTotalFull?.cost_estimate?.partial === true);
</script>

<SectionCard title="Contexte">
	<p class="text-[13px] font-medium tabular-nums">{line}</p>

	<div class="mt-3 border-t border-border pt-2">
		<p class="text-xs font-medium">Consommation session</p>
		<dl class="mt-1.5 grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 text-xs">
			<dt class="text-muted-foreground">Entrée</dt>
			<dd class="tabular-nums">{tokens(status?.totals.input_tokens)}</dd>
			<dt class="text-muted-foreground">Sortie</dt>
			<dd class="tabular-nums">{tokens(status?.totals.output_tokens)}</dd>
			<dt class="text-muted-foreground">Cache</dt>
			<dd class="tabular-nums">{tokens(status?.totals.cache_read_tokens)}</dd>
			<dt class="text-muted-foreground">Requêtes</dt>
			<dd class="tabular-nums">{status ? formatNumber(status.totals.requests) : '—'}</dd>
			<dt class="text-muted-foreground">Coût</dt>
			<dd class="tabular-nums">
				{#if totalCost !== undefined}
					<span title={partialCost ? 'Montant partiel : certaines réponses n’ont pas de prix' : undefined}
						>{formatCost(totalCost)}{#if partialCost}<span class="sr-only"> (partiel)</span>{/if}</span
					>
				{:else}
					<span class="text-muted-foreground">non mesuré</span>
				{/if}
			</dd>
		</dl>
	</div>
</SectionCard>
