<script lang="ts">
	// Long-term memory of the session (`/memory`): the space, the last recall and the
	// last maintenance pass. Read-only: no editor and no new memory engine here.
	import { onMount } from 'svelte';
	import type { InspectorState } from '$lib/state/ui.svelte';
	import * as ipc from '$lib/ipc/client';
	import { app } from '$lib/state/app.svelte';
	import { Button } from '$lib/components/ui/button';
	import { Skeleton } from '$lib/components/ui/skeleton';
	import LoaderIcon from '@lucide/svelte/icons/loader';
	import RefreshIcon from '@lucide/svelte/icons/refresh-cw';
	import {
		countOrDash,
		formatClock,
		formatTokens,
		plural,
		readMaintenanceReport
	} from './format';

	let { args }: { args: InspectorState['args'] } = $props();

	const uid = $props.id();
	const conv = $derived(app.current);
	const memory = $derived(conv?.memory ?? null);

	/** From the snapshot read on open (fresh), not from the conversation's cache. */
	let space = $state<string | null>(null);
	let snapshotRunning = $state(false);
	let loaded = $state(false);
	let loading = $state(false);
	let loadError = $state<string | null>(null);

	onMount(() => {
		void load();
	});

	async function load() {
		const current = app.current;
		if (!current) {
			loaded = true;
			return;
		}
		loading = true;
		try {
			const snap = await ipc.sessionSnapshot(current.sessionId);
			space = snap.memory_space;
			snapshotRunning = snap.maintenance_running;
			loadError = null;
		} catch (e) {
			loadError = ipc.errorMessage(e);
		} finally {
			loaded = true;
			loading = false;
		}
	}

	// The snapshot says whether a pass was running when the view opened; a finished
	// event observed since then settles it.
	const running = $derived(
		memory ? memory.maintenanceRunning || (snapshotRunning && !memory.lastMaintenance) : snapshotRunning
	);
	const report = $derived(readMaintenanceReport(memory?.lastMaintenance?.report));

	const OUTCOME: Record<string, string> = {
		completed: 'terminé',
		cancelled: 'annulé',
		failed: 'en échec'
	};
</script>

<div class="flex min-w-0 flex-col gap-5">
	<div class="flex justify-end gap-2">
		<Button variant="ghost" size="sm" onclick={load} disabled={loading}>
			<RefreshIcon class={loading ? 'animate-spin' : undefined} aria-hidden="true" />
			Actualiser
		</Button>
	</div>

	<section class="flex flex-col gap-2" aria-labelledby="{uid}-space">
		<h3 id="{uid}-space" class="text-sm font-medium">Espace de mémoire</h3>
		{#if !loaded}
			<Skeleton class="h-5 w-64" />
		{:else if loadError !== null}
			<p role="alert" class="text-sm text-destructive">
				Impossible de lire l’espace de mémoire : {loadError}
			</p>
		{:else if space !== null}
			<p class="font-mono text-sm break-all">{space}</p>
		{:else}
			<p class="text-sm text-pretty text-muted-foreground">
				La mémoire à long terme n’est pas configurée pour ce projet : activez la section [memory] dans
				bricks.toml.
			</p>
		{/if}
	</section>

	<section class="flex flex-col gap-2" aria-labelledby="{uid}-recall">
		<h3 id="{uid}-recall" class="text-sm font-medium">Dernier rappel</h3>
		{#if memory?.lastRecall}
			<p class="text-sm text-pretty tabular-nums">
				{plural(memory.lastRecall.items, 'élément rappelé', 'éléments rappelés')}, environ
				{formatTokens(memory.lastRecall.tokens)} sur {formatTokens(memory.lastRecall.budget)} autorisés.
				{#if memory.lastRecall.omitted > 0}
					{plural(
						memory.lastRecall.omitted,
						'élément écarté par le budget.',
						'éléments écartés par le budget.'
					)}
				{/if}
			</p>
			<p class="text-xs text-muted-foreground">Rappel du {formatClock(memory.lastRecall.at)}.</p>
		{:else}
			<p class="text-sm text-muted-foreground">Aucun rappel dans cette session.</p>
		{/if}
	</section>

	<section class="flex flex-col gap-2" aria-labelledby="{uid}-maintenance">
		<h3 id="{uid}-maintenance" class="text-sm font-medium">Maintenance</h3>
		{#if running}
			<p role="status" class="flex items-center gap-2 text-sm text-muted-foreground">
				<LoaderIcon class="size-4 animate-spin" aria-hidden="true" />
				Maintenance en cours.
			</p>
		{/if}

		{#if memory?.lastMaintenance}
			{@const last = memory.lastMaintenance}
			<p class="text-sm">
				Dernier passage : <span class="font-medium">{OUTCOME[last.outcome] ?? last.outcome}</span>,
				à {formatClock(last.at)}.
			</p>

			{#if report}
				<dl class="grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-1 text-sm">
					<dt class="text-muted-foreground">Épisodes extraits</dt>
					<dd class="tabular-nums">{countOrDash(report.extracted)}</dd>

					<dt class="text-muted-foreground">Faits créés</dt>
					<dd class="tabular-nums">{countOrDash(report.factsCreated)}</dd>

					<dt class="text-muted-foreground">Faits mis à jour</dt>
					<dd class="tabular-nums">{countOrDash(report.factsUpdated)}</dd>

					<dt class="text-muted-foreground">Intégrations calculées</dt>
					<dd class="tabular-nums">{countOrDash(report.embedded)}</dd>

					<dt class="text-muted-foreground">Échecs d’extraction</dt>
					<dd class="tabular-nums">{countOrDash(report.failed)}</dd>

					<dt class="text-muted-foreground">Erreurs d’intégration</dt>
					<dd class="tabular-nums">{countOrDash(report.embeddingErrors)}</dd>

					<dt class="text-muted-foreground">Restant à traiter</dt>
					<dd class="tabular-nums">{countOrDash(report.pending)}</dd>

					<dt class="text-muted-foreground">Interrompu</dt>
					<dd>{report.cancelled === null ? '—' : report.cancelled ? 'oui' : 'non'}</dd>
				</dl>

				{#if report.errors.length > 0}
					<ul class="flex list-disc flex-col gap-1 pl-4 text-xs text-pretty text-destructive">
						{#each report.errors as error, i (i)}
							<li>{error}</li>
						{/each}
					</ul>
				{/if}
			{:else if !last.error}
				<p class="text-sm text-muted-foreground">Aucun rapport détaillé pour ce passage.</p>
			{/if}

			{#if last.error}
				<p role="alert" class="text-sm text-destructive">Erreur : {last.error}</p>
			{/if}
		{:else if !running}
			<p class="text-sm text-muted-foreground">Aucun passage de maintenance dans cette session.</p>
		{/if}
	</section>
</div>
