<script lang="ts">
	// Resume picker (`/resume`, `/sessions`): the conversations of the current
	// project, and a search across every project. Choosing one reopens it in its
	// own project, through the desktop registry.
	import { onMount } from 'svelte';
	import SearchIcon from '@lucide/svelte/icons/search';
	import { Badge } from '$lib/components/ui/badge';
	import { Input } from '$lib/components/ui/input';
	import { errorMessage, type SessionItem, type SessionSearchHit } from '$lib/ipc/client';
	import EmptyState from '$lib/layout/EmptyState.svelte';
	import {
		displayTitle,
		highlightParts,
		relativeDate,
		shortPath,
		type TextPart
	} from '$lib/features/navigation/format';
	import { app } from '$lib/state/app.svelte';
	import { ui, type InspectorState } from '$lib/state/ui.svelte';

	// Opened without options: the picker always lists the current project.
	let { args: _args }: { args: InspectorState['args'] } = $props();

	const SEARCH_DELAY_MS = 250;

	let draft = $state('');
	/** The trimmed text the last search is about (empty: the project's list). */
	let query = $state('');
	let hits = $state<SessionSearchHit[]>([]);
	let searching = $state(false);
	let searchError = $state<string | null>(null);
	/** Id of the conversation being reopened, while the desktop answers. */
	let resumingId = $state<string | null>(null);
	/** Refusal of the desktop to reopen a conversation (kept in the view). */
	let refusal = $state<string | null>(null);

	const currentId = $derived(app.current?.sessionId ?? null);
	const projectSessions = $derived(app.sessions);

	onMount(() => {
		void app.refreshSessions();
	});

	// Search waits for a short pause in typing; an older answer never replaces a newer one.
	$effect(() => {
		const text = draft.trim();
		const timer = setTimeout(() => (query = text), SEARCH_DELAY_MS);
		return () => clearTimeout(timer);
	});

	$effect(() => {
		const text = query;
		if (!text) {
			hits = [];
			searching = false;
			searchError = null;
			return;
		}
		let stale = false;
		searching = true;
		searchError = null;
		app.search(text, true).then(
			(found) => {
				if (stale) return;
				hits = found;
				searching = false;
			},
			(e: unknown) => {
				if (stale) return;
				hits = [];
				searchError = errorMessage(e);
				searching = false;
			}
		);
		return () => {
			stale = true;
		};
	});

	function projectName(projectId: string | null): string {
		if (!projectId) return 'Sans projet';
		return app.projects.find((p) => p.id === projectId)?.name ?? 'Projet inconnu';
	}

	/** The title as text parts; a search hit marks the words it matched. */
	function titleParts(item: SessionItem, hit: SessionSearchHit | null): TextPart[] {
		const title = displayTitle(item.title);
		return hit ? highlightParts(title, query) : [{ text: title, match: false }];
	}

	/** One line under a title: when, size, model, folder. */
	function meta(item: SessionItem): string {
		const count = item.message_count === 1 ? '1 message' : `${item.message_count} messages`;
		const folder = item.working_dir ? shortPath(item.working_dir) : 'dossier inconnu';
		return [relativeDate(item.updated_at), count, item.model ?? 'modèle inconnu', folder].join(' · ');
	}

	async function resume(sessionId: string) {
		if (resumingId) return;
		resumingId = sessionId;
		refusal = null;
		const message = await app.resumeSession(sessionId);
		resumingId = null;
		if (message) {
			refusal = message;
			return;
		}
		ui.close();
	}

	/** Items of the list: the search hits, or the project's conversations. */
	const rows = $derived(
		query
			? hits.map((hit) => ({ item: hit.session, hit }))
			: projectSessions.map((item) => ({ item, hit: null as SessionSearchHit | null }))
	);
</script>

<div class="flex flex-col gap-3">
	<div class="relative">
		<SearchIcon
			class="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground"
			aria-hidden="true"
		/>
		<Input
			type="search"
			bind:value={draft}
			placeholder="Titre ou texte d’un message"
			aria-label="Rechercher une conversation dans tous les projets"
			class="pl-8"
			autocomplete="off"
		/>
	</div>

	{#if refusal}
		<p role="alert" class="text-sm text-destructive">{refusal}</p>
	{/if}

	{#if query && searchError}
		<p role="alert" class="text-sm text-destructive">Recherche impossible : {searchError}</p>
	{:else if query && searching && hits.length === 0}
		<p class="text-sm text-muted-foreground" aria-live="polite">Recherche en cours…</p>
	{/if}

	{#if rows.length > 0}
		<ul class="flex flex-col divide-y divide-border rounded-lg border border-border">
			{#each rows as row (row.item.id)}
				{@const item = row.item}
				<li>
					<button
						type="button"
						class="flex w-full min-w-0 flex-col gap-1 px-3 py-2.5 text-left transition-colors outline-none hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-60"
						aria-current={item.id === currentId ? 'true' : undefined}
						disabled={resumingId !== null}
						onclick={() => void resume(item.id)}
					>
						<span class="flex min-w-0 items-center gap-2">
							<span class="min-w-0 flex-1 truncate text-sm font-medium">
								{#each titleParts(item, row.hit) as part, i (i)}
									{#if part.match}<mark class="rounded-sm bg-warning-soft text-foreground">{part.text}</mark>{:else}{part.text}{/if}
								{/each}
							</span>
							{#if item.id === currentId}
								<Badge variant="default">Actuelle</Badge>
							{:else if item.open}
								<Badge variant="secondary">Ouverte</Badge>
							{/if}
							{#if item.running}
								<Badge variant="outline">En cours</Badge>
							{/if}
						</span>
						{#if row.hit?.snippet}
							<span class="line-clamp-2 text-xs text-muted-foreground">
								{#each highlightParts(row.hit.snippet, query) as part, i (i)}
									{#if part.match}<mark class="rounded-sm bg-warning-soft text-foreground">{part.text}</mark>{:else}{part.text}{/if}
								{/each}
							</span>
						{/if}
						<span class="truncate text-xs text-muted-foreground">
							{#if row.hit}{projectName(item.project_id)} · {/if}{meta(item)}
						</span>
					</button>
				</li>
			{/each}
		</ul>
	{:else if query}
		{#if !searching && !searchError}
			<EmptyState title="Aucune conversation trouvée" />
		{/if}
	{:else if !app.currentProject}
		<EmptyState
			title="Aucun projet sélectionné"
		/>
	{:else}
		<EmptyState
			title="Aucune conversation dans ce projet"
		/>
	{/if}
</div>
