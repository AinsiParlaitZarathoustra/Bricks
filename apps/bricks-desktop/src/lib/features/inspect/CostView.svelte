<script lang="ts">
	// Session consumption and cost (`/cost`, `/usage`, `details`). Figures come from
	// the engine's usage events; nothing is priced or counted again here.
	import { onMount, untrack } from 'svelte';
	import type { InspectorState } from '$lib/state/ui.svelte';
	import type { Usage } from '$lib/ipc/engine';
	import * as ipc from '$lib/ipc/client';
	import { app } from '$lib/state/app.svelte';
	import { Button } from '$lib/components/ui/button';
	import { Skeleton } from '$lib/components/ui/skeleton';
	import {
		consumedTokens,
		costCell,
		costOf,
		formatClock,
		formatInt,
		formatUsd,
		plural,
		shortText
	} from './format';

	let { args }: { args: InspectorState['args'] } = $props();

	const uid = $props.id();
	const conv = $derived(app.current);
	let showDetails = $state(untrack(() => args.details === true || args.details === 'true'));

	/** Totals read from the snapshot when the view opens (used until a live usage arrives). */
	let openedUsage = $state<Usage | null>(null);
	let loaded = $state(false);
	let loadError = $state<string | null>(null);

	onMount(() => {
		void loadOpened();
	});

	async function loadOpened() {
		const current = app.current;
		if (!current) {
			loaded = true;
			return;
		}
		try {
			openedUsage = (await ipc.sessionSnapshot(current.sessionId)).usage;
		} catch (e) {
			loadError = ipc.errorMessage(e);
		} finally {
			loaded = true;
		}
	}

	const live = $derived(Boolean(conv?.usageTotalFull));
	const total = $derived<Usage | null>(conv?.usageTotalFull ?? openedUsage);
	const consumed = $derived(consumedTokens(total) > 0);
	const cost = $derived(costOf(total));
	const requests = $derived(conv?.context?.totals ?? null);
	const runRows = $derived(conv ? [...conv.runUsages.entries()] : []);
	const turns = $derived(conv?.turnUsages ?? []);

	function runPrompt(runId: string | undefined): string {
		if (!runId) return '—';
		const prompt = conv?.runs.get(runId)?.prompt;
		return prompt ? shortText(prompt, 60) : '(question non disponible)';
	}
</script>

<div class="flex min-w-0 flex-col gap-5">
	<div class="flex justify-end gap-2">
		<Button
			variant="ghost"
			size="sm"
			aria-expanded={showDetails}
			onclick={() => (showDetails = !showDetails)}
		>
			{showDetails ? 'Masquer le détail' : 'Voir le détail par réponse'}
		</Button>
	</div>

	<section class="flex flex-col gap-2" aria-labelledby="{uid}-session">
		<h3 id="{uid}-session" class="text-sm font-medium">Session</h3>
		{#if !loaded && !live}
			<Skeleton class="h-24 w-full" />
		{:else if !total || !consumed}
			<p class="text-sm text-muted-foreground">
				Aucune consommation observée.
			</p>
		{:else}
			<dl class="grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-1.5 text-sm">
				<dt class="text-muted-foreground">Requêtes</dt>
				<dd class="tabular-nums">
					{requests ? formatInt(requests.requests) : '—'}
					{#if requests && requests.compaction_requests > 0}
						<span class="text-muted-foreground"
							>dont {formatInt(requests.compaction_requests)} pour la compaction</span
						>
					{/if}
					{#if requests && requests.requests_without_usage > 0}
						<span class="text-muted-foreground"
							>({formatInt(requests.requests_without_usage)} sans compte de tokens)</span
						>
					{/if}
				</dd>

				<dt class="text-muted-foreground">Entrée</dt>
				<dd class="tabular-nums">{formatInt(total.input_tokens)} tokens</dd>

				<dt class="text-muted-foreground">Sortie</dt>
				<dd class="tabular-nums">{formatInt(total.output_tokens)} tokens</dd>

				<dt class="text-muted-foreground">dont raisonnement</dt>
				<dd class="tabular-nums">{formatInt(total.reasoning_tokens)} tokens</dd>

				<dt class="text-muted-foreground">Lecture du cache</dt>
				<dd class="tabular-nums">{formatInt(total.cache_read_input_tokens)} tokens</dd>

				<dt class="text-muted-foreground">Écriture du cache</dt>
				<dd class="tabular-nums">{formatInt(total.cache_creation_input_tokens)} tokens</dd>

				<dt class="text-muted-foreground">Total</dt>
				<dd class="tabular-nums">{formatInt(total.total_tokens)} tokens</dd>

				<dt class="text-muted-foreground">Coût</dt>
				<dd>
					{#if cost.kind === 'exact'}
						<span class="tabular-nums">{formatUsd(cost.usd)}</span>
						<span class="block text-xs text-pretty text-muted-foreground"
							>Estimation d’après les tarifs configurés, pas une facture.</span
						>
					{:else if cost.kind === 'partial'}
						<span class="tabular-nums">Au moins {formatUsd(cost.usd)}</span>
						<span class="block text-xs text-pretty text-muted-foreground"
							>Sous-total partiel : pas de tarif pour
							{cost.unpriced.length > 0 ? cost.unpriced.join(', ') : 'une partie de la consommation'}.</span
						>
					{:else}
						<span>Coût inconnu (aucun tarif configuré)</span>
					{/if}
				</dd>
			</dl>

			
		{/if}
		{#if loadError && !total}
			<p role="alert" class="text-sm text-destructive">Impossible de relire les totaux : {loadError}</p>
		{/if}
	</section>

	<section class="flex flex-col gap-2" aria-labelledby="{uid}-runs">
		<h3 id="{uid}-runs" class="text-sm font-medium">Par tour (run racine)</h3>
		{#if runRows.length === 0}
			<p class="text-sm text-muted-foreground">Aucun tour compté.</p>
		{:else}
			{#each runRows as [rootId, ru] (rootId)}
				<div class="flex min-w-0 flex-col gap-2 rounded-md border border-border p-3">
					<p class="text-sm text-pretty">
						<span class="text-muted-foreground">Tour :</span>
						{runPrompt(rootId)}
					</p>
					<div class="overflow-x-auto">
						<table class="w-full min-w-[22rem] text-xs tabular-nums">
							<thead>
								<tr class="text-muted-foreground">
									<th scope="col" class="py-1 text-left font-normal"><span class="sr-only">Mesure</span></th>
									<th scope="col" class="py-1 text-right font-normal">Propre</th>
									<th scope="col" class="py-1 text-right font-normal">Sous-agents</th>
									<th scope="col" class="py-1 text-right font-normal">Total du tour</th>
								</tr>
							</thead>
							<tbody>
								<tr class="border-t border-border">
									<th scope="row" class="py-1 text-left font-normal text-muted-foreground">Entrée</th>
									<td class="py-1 text-right">{formatInt(ru.own.input_tokens)}</td>
									<td class="py-1 text-right">{formatInt(ru.descendants.input_tokens)}</td>
									<td class="py-1 text-right">{formatInt(ru.total.input_tokens)}</td>
								</tr>
								<tr class="border-t border-border">
									<th scope="row" class="py-1 text-left font-normal text-muted-foreground">Sortie</th>
									<td class="py-1 text-right">{formatInt(ru.own.output_tokens)}</td>
									<td class="py-1 text-right">{formatInt(ru.descendants.output_tokens)}</td>
									<td class="py-1 text-right">{formatInt(ru.total.output_tokens)}</td>
								</tr>
								<tr class="border-t border-border">
									<th scope="row" class="py-1 text-left font-normal text-muted-foreground">Total</th>
									<td class="py-1 text-right">{formatInt(ru.own.total_tokens)}</td>
									<td class="py-1 text-right">{formatInt(ru.descendants.total_tokens)}</td>
									<td class="py-1 text-right">{formatInt(ru.total.total_tokens)}</td>
								</tr>
								<tr class="border-t border-border">
									<th scope="row" class="py-1 text-left font-normal text-muted-foreground">Coût</th>
									<td class="py-1 text-right">{costCell(costOf(ru.own))}</td>
									<td class="py-1 text-right">{costCell(costOf(ru.descendants))}</td>
									<td class="py-1 text-right">{costCell(costOf(ru.total))}</td>
								</tr>
							</tbody>
						</table>
					</div>
					{#if !ru.final_total}
						<p class="text-xs text-pretty text-warning">
							Total provisoire : {plural(
								ru.pending_agents,
								'sous-agent encore en cours',
								'sous-agents encore en cours'
							)}. 
						</p>
					{/if}
				</div>
			{/each}
			
		{/if}
	</section>

	{#if showDetails}
		<section class="flex flex-col gap-2" aria-labelledby="{uid}-turns">
			<h3 id="{uid}-turns" class="text-sm font-medium">Par réponse</h3>
			{#if turns.length === 0}
				<p class="text-sm text-pretty text-muted-foreground">
					Aucune réponse reçue depuis l’ouverture de la session.
				</p>
			{:else}
				<div class="overflow-x-auto">
					<table class="w-full min-w-[32rem] text-xs tabular-nums">
						<thead>
							<tr class="text-muted-foreground">
								<th scope="col" class="py-1 text-left font-normal">#</th>
								<th scope="col" class="py-1 text-left font-normal">Heure</th>
								<th scope="col" class="py-1 text-left font-normal">Tour</th>
								<th scope="col" class="py-1 text-right font-normal">Entrée</th>
								<th scope="col" class="py-1 text-right font-normal">Sortie</th>
								<th scope="col" class="py-1 text-right font-normal">Cache lu</th>
								<th scope="col" class="py-1 text-right font-normal">Coût</th>
							</tr>
						</thead>
						<tbody>
							{#each turns as turn, i (i)}
								<tr class="border-t border-border">
									<td class="py-1 text-muted-foreground">{i + 1}</td>
									<td class="py-1">{formatClock(turn.at)}</td>
									<td class="max-w-[12rem] truncate py-1" title={runPrompt(turn.runId)}>
										{runPrompt(turn.runId)}
									</td>
									<td class="py-1 text-right">{formatInt(turn.usage.input_tokens)}</td>
									<td class="py-1 text-right">{formatInt(turn.usage.output_tokens)}</td>
									<td class="py-1 text-right">{formatInt(turn.usage.cache_read_input_tokens)}</td>
									<td class="py-1 text-right">{costCell(costOf(turn.usage))}</td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
				<p class="text-xs text-pretty text-muted-foreground">
					{plural(turns.length, 'réponse affichée', 'réponses affichées')}{#if turns.length >= 2000} · seules les 2 000 dernières sont conservées{/if}
				</p>
			{/if}
		</section>
	{/if}
</div>
