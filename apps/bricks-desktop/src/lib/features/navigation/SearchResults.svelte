<script lang="ts">
	import { app } from '$lib/state/app.svelte';
	import { errorMessage } from '$lib/ipc/client';
	import type { SessionSearchHit } from '$lib/ipc/client';
	import { createClock } from './clock.svelte';
	import { displayTitle, highlightParts, relativeDate } from './format';

	interface Props {
		/** The text being searched; cleared once a result is opened. */
		query?: string;
		/** Called once a result is opened. */
		onopened?: () => void;
	}

	let { query = $bindable(''), onopened }: Props = $props();

	const DEBOUNCE_MS = 250;
	const clock = createClock();

	// The text actually sent to the engine, once typing has paused for DEBOUNCE_MS.
	let debounced = $state('');
	let hits = $state<SessionSearchHit[]>([]);
	let loading = $state(false);
	let failure = $state<string | null>(null);

	const pending = $derived(query.trim() !== debounced);
	const countLabel = $derived(hits.length === 1 ? '1 résultat' : `${hits.length} résultats`);

	// Debounce: the query is sent only once typing has paused.
	$effect(() => {
		const next = query.trim();
		const timer = setTimeout(
			() => {
				debounced = next;
			},
			next ? DEBOUNCE_MS : 0
		);
		return () => clearTimeout(timer);
	});

	// Identifies the latest request, so a slow earlier response never overwrites a newer one.
	let request = 0;

	// Re-runs when the debounced text changes. The search covers every project.
	$effect(() => {
		void runSearch(debounced);
	});

	async function runSearch(text: string) {
		const mine = ++request;
		if (!text) {
			hits = [];
			failure = null;
			loading = false;
			return;
		}
		loading = true;
		try {
			const found = await app.search(text, true);
			if (mine !== request) return;
			hits = found;
			failure = null;
		} catch (error) {
			if (mine !== request) return;
			hits = [];
			failure = errorMessage(error);
		} finally {
			if (mine === request) loading = false;
		}
	}

	function projectName(hit: SessionSearchHit): string {
		const projectId = hit.session.project_id;
		return app.projects.find((p) => p.id === projectId)?.name ?? 'Projet inconnu';
	}

	async function openHit(hit: SessionSearchHit) {
		const session = hit.session;
		const projectId = session.project_id;
		if (projectId && projectId !== app.currentProjectId) {
			const project = app.projects.find((p) => p.id === projectId);
			if (!project) {
				app.fail(new Error('Le projet de cette conversation est introuvable.'));
				return;
			}
			if (app.requireFolder(projectId, () => openHit(hit))) {
				onopened?.();
				return;
			}
			await app.selectProject(projectId);
		}
		query = '';
		onopened?.();
		await app.openSession(session.id);
	}
</script>

<!-- Text is rendered as nodes, never as HTML: titles and messages are untrusted. -->
{#snippet highlighted(text: string)}
	{#each highlightParts(text, debounced) as part, index (index)}
		{#if part.match}
			<mark class="rounded-sm bg-warning-soft px-0.5 text-foreground">{part.text}</mark>
		{:else}
			{part.text}
		{/if}
	{/each}
{/snippet}

<section aria-label="Résultats de recherche" class="flex flex-col gap-1 px-2 pb-3">
	{#if pending || loading}
		<p role="status" class="px-2 py-2 text-xs text-muted-foreground">Recherche en cours…</p>
	{:else if failure}
		<p role="alert" class="px-2 py-2 text-xs text-destructive">Recherche impossible : {failure}</p>
	{:else if hits.length === 0}
		<p role="status" class="px-2 py-2 text-sm text-muted-foreground">Aucun résultat pour « {debounced} ».</p>
	{:else}
		<p role="status" class="px-2 text-xs text-muted-foreground">{countLabel}</p>
		<ul class="flex flex-col gap-0.5">
			{#each hits as hit (hit.session.id)}
				<li>
					<button
						type="button"
						class="flex w-full min-w-0 flex-col items-start gap-1 rounded-md px-2 py-2 text-left hover:bg-sidebar-accent/60"
						onclick={() => openHit(hit)}
					>
						<span class="flex w-full min-w-0 items-center gap-2">
							<span class="min-w-0 flex-1 truncate text-sm font-medium">
								{@render highlighted(displayTitle(hit.session.title))}
							</span>
							<span class="shrink-0 text-xs text-muted-foreground">
								{relativeDate(hit.session.updated_at, clock.now)}
							</span>
						</span>
						<span class="flex w-full min-w-0 items-center gap-1.5 text-xs text-muted-foreground">
							<span class="truncate">{projectName(hit)}</span>
							<span aria-hidden="true">·</span>
							<span class="shrink-0">{hit.matched === 'title' ? 'Dans le titre' : 'Dans un message'}</span>
						</span>
						{#if hit.matched === 'message' && hit.snippet}
							<span class="line-clamp-2 text-xs text-muted-foreground">
								{@render highlighted(hit.snippet)}
							</span>
						{/if}
					</button>
				</li>
			{/each}
		</ul>
	{/if}
</section>
