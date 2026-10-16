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
</script>

<SectionCard title="Contexte">
	<p class="text-sm font-medium tabular-nums">{line}</p>

	<div class="mt-3 border-t border-border pt-2">
		<p class="text-xs font-medium">Consommation cumulée (facturée)</p>
		{#if status}
			<dl class="mt-1.5 grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 text-xs">
				<dt class="text-muted-foreground">Entrée</dt>
				<dd class="tabular-nums">{formatTokens(status.totals.input_tokens)}</dd>
				<dt class="text-muted-foreground">Sortie</dt>
				<dd class="tabular-nums">{formatTokens(status.totals.output_tokens)}</dd>
				<dt class="text-muted-foreground">Lecture du cache</dt>
				<dd class="tabular-nums">{formatTokens(status.totals.cache_read_tokens)}</dd>
				<dt class="text-muted-foreground">Écriture du cache</dt>
				<dd class="tabular-nums">{formatTokens(status.totals.cache_write_tokens)}</dd>
				<dt class="text-muted-foreground">Raisonnement</dt>
				<dd class="tabular-nums">{formatTokens(status.totals.reasoning_tokens)}</dd>
				<dt class="text-muted-foreground">Requêtes</dt>
				<dd class="tabular-nums">
					{formatNumber(status.totals.requests)}
					{#if status.totals.requests_without_usage > 0}
						<span class="text-muted-foreground"
							>({formatNumber(status.totals.requests_without_usage)} sans compte de tokens)</span
						>
					{/if}
				</dd>
				<dt class="text-muted-foreground">Coût</dt>
				<dd class="tabular-nums">
					{#if totalCost !== undefined}
						{formatCost(totalCost)}
					{:else if conversation.usageTotal}
						<span class="text-muted-foreground">non tarifé</span>
					{:else}
						<span class="text-muted-foreground">non mesuré</span>
					{/if}
				</dd>
			</dl>
		{:else}
			<p class="mt-1 text-xs text-muted-foreground">Aucune requête pour l’instant.</p>
		{/if}
	</div>
</SectionCard>
