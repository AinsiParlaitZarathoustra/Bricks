<script lang="ts">
	// Session consumption and cost (`/cost`, `/usage`, `details`). Figures come from
	// the engine's usage events; nothing is priced or counted again here.
	import { t } from '$lib/i18n/i18n.svelte';
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
		return prompt ? shortText(prompt, 60) : t('inspect.questionUnavailable');
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
			{showDetails ? t('inspect.hideTheDetails') : t('inspect.viewTheDetailsPerAnswer')}
		</Button>
	</div>

	<section class="flex flex-col gap-2" aria-labelledby="{uid}-session">
		<h3 id="{uid}-session" class="text-sm font-medium">{t('common.session')}</h3>
		{#if !loaded && !live}
			<Skeleton class="h-24 w-full" />
		{:else if !total || !consumed}
			<p class="text-sm text-muted-foreground">
				{t('inspect.noUsageObserved')}
			</p>
		{:else}
			<dl class="grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-1.5 text-sm">
				<dt class="text-muted-foreground">{t('inspect.requests')}</dt>
				<dd class="tabular-nums">
					{requests ? formatInt(requests.requests) : '—'}
					{#if requests && requests.compaction_requests > 0}
						<span class="text-muted-foreground"
							>{t('inspect.includingForCompaction', { compactionRequests: formatInt(requests.compaction_requests) })}</span
						>
					{/if}
					{#if requests && requests.requests_without_usage > 0}
						<span class="text-muted-foreground"
							>{t('inspect.withoutTokenCount', { requestsWithoutUsage: formatInt(requests.requests_without_usage) })}</span
						>
					{/if}
				</dd>

				<dt class="text-muted-foreground">{t('common.enter')}</dt>
				<dd class="tabular-nums">{t('common.tokens', { tokens: formatInt(total.input_tokens) })}</dd>

				<dt class="text-muted-foreground">{t('common.output')}</dt>
				<dd class="tabular-nums">{t('common.tokens', { tokens: formatInt(total.output_tokens) })}</dd>

				<dt class="text-muted-foreground">{t('inspect.includingReasoning')}</dt>
				<dd class="tabular-nums">{t('common.tokens', { tokens: formatInt(total.reasoning_tokens) })}</dd>

				<dt class="text-muted-foreground">{t('inspect.cacheRead')}</dt>
				<dd class="tabular-nums">{t('common.tokens', { tokens: formatInt(total.cache_read_input_tokens) })}</dd>

				<dt class="text-muted-foreground">{t('inspect.cacheWrite')}</dt>
				<dd class="tabular-nums">{t('common.tokens', { tokens: formatInt(total.cache_creation_input_tokens) })}</dd>

				<dt class="text-muted-foreground">{t('inspect.total')}</dt>
				<dd class="tabular-nums">{t('common.tokens', { tokens: formatInt(total.total_tokens) })}</dd>

				<dt class="text-muted-foreground">{t('inspect.cost')}</dt>
				<dd>
					{#if cost.kind === 'exact'}
						<span class="tabular-nums">{formatUsd(cost.usd)}</span>
					{:else if cost.kind === 'partial'}
						<span class="tabular-nums">{t('inspect.atLeast', { usd: formatUsd(cost.usd) })}</span>
					{:else}
						<span>{t('inspect.unknownCostNoPricingConfigured')}</span>
					{/if}
				</dd>
			</dl>

			
		{/if}
		{#if loadError && !total}
			<p role="alert" class="text-sm text-destructive">{t('inspect.couldNotReloadTheTotals', { loadError })}</p>
		{/if}
	</section>

	<section class="flex flex-col gap-2" aria-labelledby="{uid}-runs">
		<h3 id="{uid}-runs" class="text-sm font-medium">{t('inspect.perTurnRootRun')}</h3>
		{#if runRows.length === 0}
			<p class="text-sm text-muted-foreground">{t('inspect.noTurnCounted')}</p>
		{:else}
			{#each runRows as [rootId, ru] (rootId)}
				<div class="flex min-w-0 flex-col gap-2 rounded-md border border-border p-3">
					<p class="text-sm text-pretty">
						<span class="text-muted-foreground">{t('inspect.turn')}</span>
						{runPrompt(rootId)}
					</p>
					<div class="overflow-x-auto">
						<table class="w-full min-w-[22rem] text-xs tabular-nums">
							<thead>
								<tr class="text-muted-foreground">
									<th scope="col" class="py-1 text-left font-normal"><span class="sr-only">{t('inspect.measure')}</span></th>
									<th scope="col" class="py-1 text-right font-normal">{t('inspect.own')}</th>
									<th scope="col" class="py-1 text-right font-normal">{t('common.subAgents')}</th>
									<th scope="col" class="py-1 text-right font-normal">{t('inspect.turnTotal')}</th>
								</tr>
							</thead>
							<tbody>
								<tr class="border-t border-border">
									<th scope="row" class="py-1 text-left font-normal text-muted-foreground">{t('common.enter')}</th>
									<td class="py-1 text-right">{formatInt(ru.own.input_tokens)}</td>
									<td class="py-1 text-right">{formatInt(ru.descendants.input_tokens)}</td>
									<td class="py-1 text-right">{formatInt(ru.total.input_tokens)}</td>
								</tr>
								<tr class="border-t border-border">
									<th scope="row" class="py-1 text-left font-normal text-muted-foreground">{t('common.output')}</th>
									<td class="py-1 text-right">{formatInt(ru.own.output_tokens)}</td>
									<td class="py-1 text-right">{formatInt(ru.descendants.output_tokens)}</td>
									<td class="py-1 text-right">{formatInt(ru.total.output_tokens)}</td>
								</tr>
								<tr class="border-t border-border">
									<th scope="row" class="py-1 text-left font-normal text-muted-foreground">{t('inspect.total')}</th>
									<td class="py-1 text-right">{formatInt(ru.own.total_tokens)}</td>
									<td class="py-1 text-right">{formatInt(ru.descendants.total_tokens)}</td>
									<td class="py-1 text-right">{formatInt(ru.total.total_tokens)}</td>
								</tr>
								<tr class="border-t border-border">
									<th scope="row" class="py-1 text-left font-normal text-muted-foreground">{t('inspect.cost')}</th>
									<td class="py-1 text-right">{costCell(costOf(ru.own))}</td>
									<td class="py-1 text-right">{costCell(costOf(ru.descendants))}</td>
									<td class="py-1 text-right">{costCell(costOf(ru.total))}</td>
								</tr>
							</tbody>
						</table>
					</div>
					{#if !ru.final_total}
						<p class="text-xs text-pretty text-warning">
							{t('inspect.provisionalTotal', { count: ru.pending_agents })} 
						</p>
					{/if}
				</div>
			{/each}
			
		{/if}
	</section>

	{#if showDetails}
		<section class="flex flex-col gap-2" aria-labelledby="{uid}-turns">
			<h3 id="{uid}-turns" class="text-sm font-medium">{t('inspect.perAnswer')}</h3>
			{#if turns.length === 0}
				<p class="text-sm text-pretty text-muted-foreground">
					{t('inspect.noAnswerReceivedSinceTheSession')}
				</p>
			{:else}
				<div class="overflow-x-auto">
					<table class="w-full min-w-[32rem] text-xs tabular-nums">
						<thead>
							<tr class="text-muted-foreground">
								<th scope="col" class="py-1 text-left font-normal">#</th>
								<th scope="col" class="py-1 text-left font-normal">{t('inspect.time')}</th>
								<th scope="col" class="py-1 text-left font-normal">{t('inspect.turn2')}</th>
								<th scope="col" class="py-1 text-right font-normal">{t('common.enter')}</th>
								<th scope="col" class="py-1 text-right font-normal">{t('common.output')}</th>
								<th scope="col" class="py-1 text-right font-normal">{t('inspect.cacheRead2')}</th>
								<th scope="col" class="py-1 text-right font-normal">{t('inspect.cost')}</th>
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
					{t('inspect.answersShown', { count: turns.length })}
				</p>
			{/if}
		</section>
	{/if}
</div>
