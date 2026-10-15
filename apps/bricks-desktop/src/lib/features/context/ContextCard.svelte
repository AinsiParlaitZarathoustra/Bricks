<script lang="ts">
	import type { Conversation } from '$lib/state/conversation.svelte';
	import { app } from '$lib/state/app.svelte';
	import SectionCard from '$lib/layout/SectionCard.svelte';
	import ContextRing from './ContextRing.svelte';
	import {
		PROVENANCE_LABEL,
		contextView,
		formatCost,
		formatNumber,
		formatTokens
	} from './contextView';

	interface Props {
		conversation: Conversation;
	}

	let { conversation }: Props = $props();

	const status = $derived(conversation.context);
	const stale = $derived(conversation.contextStale);
	const view = $derived(contextView(status, stale));
	const modelName = $derived(
		app.model(conversation.model)?.name ?? (conversation.model || 'modèle inconnu')
	);

	const summary = $derived.by(() => {
		if (!status) return 'Aucune mesure pour l’instant.';
		if (view.kind === 'stale')
			return 'Le modèle, la compaction ou le vidage ont changé le contexte. La mesure suivante sera affichée au prochain tour.';
		if (view.kind === 'unavailable') {
			if (view.estimate !== null) return `Estimation : ~${formatTokens(view.estimate)}`;
			if (view.capacity === null) return 'Capacité du modèle inconnue.';
			return 'Mesure exacte indisponible pour ce contexte.';
		}
		return `${formatTokens(status.context_used.tokens)} sur ${formatTokens(view.capacity ?? 0)}`;
	});

	const capacityText = $derived.by(() => {
		if (!status) return '—';
		if (view.capacityKind === 'window')
			return `Fenêtre de contexte : ${formatTokens(status.context_window.total ?? 0)} (${modelName})`;
		if (view.capacityKind === 'input_limit')
			return `Limite d’entrée : ${formatTokens(status.input_limit)}. Ce n’est pas la fenêtre totale du modèle.`;
		return 'Inconnue';
	});

	const totalCost = $derived(conversation.usageTotal?.costUsd);
</script>

<SectionCard title="Contexte" description="Occupation du contexte pour le prochain tour.">
	<div class="flex items-center gap-3">
		<ContextRing {status} {stale} size={44} />
		<div class="min-w-0 flex-1">
			<p
				class={[
					'text-sm font-medium',
					view.kind === 'overflow' ? 'text-warning' : 'text-foreground'
				]}
			>
				{#if view.kind === 'overflow'}
					Dépassement
					<span class="font-mono text-xs tabular-nums">({Math.round(view.percent ?? 0)}&nbsp;%)</span>
				{:else}
					{view.headline}
				{/if}
			</p>
			<p class="text-xs text-pretty text-muted-foreground">{summary}</p>
		</div>
	</div>

	{#if status && !stale}
		<dl class="mt-3 grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1.5 text-xs">
			<dt class="text-muted-foreground">Occupation</dt>
			<dd class="tabular-nums">{formatTokens(status.context_used.tokens)}</dd>

			<dt class="text-muted-foreground">Capacité</dt>
			<dd class="min-w-0">{capacityText}</dd>

			<dt class="text-muted-foreground">Sortie réservée</dt>
			<dd class="tabular-nums">{formatTokens(status.reserved_output)}</dd>

			<dt class="text-muted-foreground">Mesure</dt>
			<dd class="min-w-0">
				{PROVENANCE_LABEL[status.context_used.provenance]}
				<span class="block font-mono text-xs break-all text-muted-foreground"
					>{status.context_used.method}</span
				>
			</dd>

			{#if status.context_used.estimated_tokens > 0}
				<dt class="text-muted-foreground">Détail</dt>
				<dd class="tabular-nums">
					{formatNumber(status.context_used.measured_tokens)} mesurés,
					{formatNumber(status.context_used.estimated_tokens)} estimés
				</dd>
			{/if}
		</dl>

		{#if status.notes.length > 0}
			<ul class="mt-3 flex list-disc flex-col gap-1 pl-4 text-xs text-pretty text-muted-foreground">
				{#each status.notes as note, i (i)}
					<li>{note}</li>
				{/each}
			</ul>
		{/if}
	{/if}

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
		<p class="mt-2 text-xs text-pretty text-muted-foreground">
			Ces totaux additionnent toutes les requêtes de la session. Ils ne mesurent pas l’occupation du
			contexte.
		</p>
	</div>
</SectionCard>
