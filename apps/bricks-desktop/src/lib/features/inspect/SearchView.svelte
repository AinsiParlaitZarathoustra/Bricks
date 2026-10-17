<script lang="ts">
	// Workspace text search (`/search`, `/s`): a real search of the project's
	// files by the engine. Not the conversation search.
	import { onMount, untrack } from 'svelte';
	import type { InspectorState } from '$lib/state/ui.svelte';
	import { app } from '$lib/state/app.svelte';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Label } from '$lib/components/ui/label';
	import { Switch } from '$lib/components/ui/switch';
	import LoaderIcon from '@lucide/svelte/icons/loader';
	import SearchIcon from '@lucide/svelte/icons/search';
	import { cn } from '$lib/utils.js';
	import { formatInt, formatMs, plural } from './format';
	import { groupByFile, highlight, readSearchInput, searchStatus } from './search';

	let { args }: { args: InspectorState['args'] } = $props();

	const uid = $props.id();
	const conv = $derived(app.current);
	const result = $derived(conv?.searchResult ?? null);
	const groups = $derived(result ? groupByFile(result.hits) : []);
	const status = $derived(result ? searchStatus(result.status) : null);

	// The query given with the command: read once, when the view opens.
	const initialQuery = untrack(() => (typeof args.query === 'string' ? args.query : ''));
	let text = $state(initialQuery);
	let regexSwitch = $state(false);
	let formError = $state<string | null>(null);

	/** The request sent last, with the markers of the answers already in the reducer. */
	let sent = $state<{
		text: string;
		regex: boolean;
		resultAt: number | null;
		rejectionAt: number | null;
	} | null>(null);

	// The engine answers with an event: the view waits for a result newer than
	// the one it had when it sent, or for a refusal of `search` newer than the
	// last rejection it saw.
	const answered = $derived(
		sent !== null && result !== null && (sent.resultAt === null || result.at > sent.resultAt)
	);
	const refusal = $derived.by(() => {
		const r = conv?.lastRejection;
		if (!sent || !r || r.command !== 'search') return null;
		return sent.rejectionAt === null || r.at > sent.rejectionAt ? r.reason : null;
	});
	const pending = $derived(sent !== null && !answered && refusal === null);
	// The highlight follows the mode of the request that produced the shown result.
	const highlightRegex = $derived(sent !== null && answered ? sent.regex : false);

	const preview = $derived(readSearchInput(text, regexSwitch));
	const hasText = $derived(preview.text.trim() !== '');

	async function submit(event?: SubmitEvent) {
		event?.preventDefault();
		const current = app.current;
		if (!current) {
			formError = 'Ouvrez une session : la recherche porte sur le dossier du projet.';
			return;
		}
		const request = readSearchInput(text, regexSwitch);
		if (request.text.trim() === '') {
			formError = 'Saisissez le texte à chercher.';
			return;
		}
		formError = null;
		sent = {
			text: request.text,
			regex: request.regex,
			resultAt: current.searchResult?.at ?? null,
			rejectionAt: current.lastRejection?.at ?? null
		};
		const refused = await app.command(current.sessionId, {
			type: 'search',
			text: request.text,
			regex: request.regex
		});
		if (refused !== null) {
			sent = null;
			formError = refused;
		}
	}

	// A query given with the command (`/search words`) runs once, as the TUI does.
	onMount(() => {
		if (initialQuery.trim() !== '') void submit();
	});
</script>

<div class="flex min-w-0 flex-col gap-4">
	<form class="flex flex-col gap-2" onsubmit={submit}>
		<Label for="{uid}-query">Texte à chercher dans les fichiers du projet</Label>
		<div class="flex gap-2">
			<Input
				id="{uid}-query"
				bind:value={text}
				placeholder="Texte, ou re:expression"
				autocomplete="off"
				spellcheck={false}
				class="font-mono"
			/>
			<Button type="submit" disabled={pending || !hasText}>
				{#if pending}
					<LoaderIcon class="animate-spin" aria-hidden="true" />
					Recherche…
				{:else}
					<SearchIcon aria-hidden="true" />
					Rechercher
				{/if}
			</Button>
		</div>
		<div class="flex flex-wrap items-center gap-x-4 gap-y-2">
			<div class="flex items-center gap-2">
				<Switch id="{uid}-regex" bind:checked={regexSwitch} />
				<Label for="{uid}-regex" class="font-normal">Expression régulière</Label>
			</div>
		</div>
		{#if formError}
			<p role="alert" class="text-sm text-destructive">{formError}</p>
		{/if}
	</form>

	<div role="status" aria-live="polite" class="text-sm">
		{#if pending}
			<p class="flex items-center gap-2 text-muted-foreground">
				<LoaderIcon class="size-4 animate-spin" aria-hidden="true" />
				Recherche en cours dans {conv?.workingDir ?? 'le projet'}…
			</p>
		{:else if refusal !== null}
			<p role="alert" class="text-destructive">Recherche refusée : {refusal}</p>
		{/if}
	</div>

	{#if !pending && result && status}
		<section class="flex min-w-0 flex-col gap-3" aria-label="Résultats de la recherche">
			<header class="flex min-w-0 flex-col gap-1">
				<div class="flex min-w-0 flex-wrap items-baseline gap-x-2 gap-y-1">
					<h3 class="min-w-0 text-sm font-medium break-all">
						Résultats pour « <span class="font-mono">{result.query}</span> »
					</h3>
					<span
						class={cn(
							'text-xs font-medium',
							status.tone === 'success' && 'text-success',
							status.tone === 'warning' && 'text-warning',
							status.tone === 'destructive' && 'text-destructive',
							status.tone === 'muted' && 'text-muted-foreground'
						)}>{status.label}</span
					>
				</div>
				<p class="text-xs text-muted-foreground tabular-nums">
					{plural(result.hits.length, 'résultat affiché', 'résultats affichés')}
					dans {plural(groups.length, 'fichier', 'fichiers')} · {formatMs(result.elapsedMs)}
				</p>
			</header>

			{#if result.notes.length > 0}
				<ul class="flex list-disc flex-col gap-1 pl-4 text-xs text-pretty text-muted-foreground">
					{#each result.notes as note, i (i)}
						<li>{note}</li>
					{/each}
				</ul>
			{/if}

			{#if groups.length === 0}
				<p class="text-sm text-pretty text-muted-foreground">
					{#if result.status === 'complete'}
						Aucun résultat dans le texte des fichiers du projet.
					{:else if result.status === 'partial'}
						Aucun résultat affiché, mais la recherche est partielle : l’absence ne prouve rien.
					{:else}
						Aucun résultat affiché.
					{/if}
				</p>
			{:else}
				<div class="flex flex-col gap-3">
					{#each groups as group (group.path)}
						<div class="flex min-w-0 flex-col gap-1">
							<h4 class="flex min-w-0 items-baseline gap-2 text-xs font-medium">
								<span class="min-w-0 truncate font-mono" title={group.path}>{group.path}</span>
								<span class="shrink-0 text-muted-foreground tabular-nums"
									>{formatInt(group.hits.length)}</span
								>
							</h4>
							<ul class="flex flex-col divide-y divide-border rounded-md border border-border">
								{#each group.hits as hit, i (i)}
									<li class="flex min-w-0 gap-3 px-2 py-1.5 text-xs">
										<span class="shrink-0 font-mono text-muted-foreground tabular-nums"
											>{hit.line}:{hit.column}</span
										>
										<code class="min-w-0 font-mono break-all whitespace-pre-wrap"
											>{#each highlight(hit.text, result.query, highlightRegex) as segment, j (j)}{#if segment.match}<mark
														class="rounded-sm bg-warning-soft text-foreground">{segment.text}</mark
													>{:else}{segment.text}{/if}{/each}</code
										>
									</li>
								{/each}
							</ul>
						</div>
					{/each}
				</div>
			{/if}

			{#if result.omitted > 0}
				<p class="text-xs text-muted-foreground tabular-nums">
					{plural(
						result.omitted,
						'occurrence trouvée n’est pas affichée',
						'occurrences trouvées ne sont pas affichées'
					)}.
				</p>
			{/if}
		</section>
	{/if}
</div>
