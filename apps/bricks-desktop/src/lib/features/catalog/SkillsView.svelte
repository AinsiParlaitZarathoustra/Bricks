<script lang="ts">
	import { onMount } from 'svelte';
	import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';
	import SearchIcon from '@lucide/svelte/icons/search';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { plural } from '$lib/features/agents/agentView';
	import { app } from '$lib/state/app.svelte';
	import type { InspectorState } from '$lib/state/ui.svelte';
	import Pager from './Pager.svelte';
	import { ENGINE_PAGE_SIZE, pageCount, scopeLabel, skillStateView } from './labels';
	import { EngineRequest, skillsSlot, type SkillsAnswer } from './request.svelte';

	// Opened without options: the view always lists the session's skills.
	let { args: _args }: { args: InspectorState['args'] } = $props();

	const conv = $derived(app.current);
	const request = new EngineRequest();
	const trust = new EngineRequest();
	/** Text of the search box, and the query of the last search sent. */
	let draft = $state('');
	let query = $state('');

	/** The engine's answer to the last listing request; null while awaited. */
	const answer = $derived(request.answered<SkillsAnswer>());
	const listing = $derived(answer ? answer.listing : null);

	/** Project decision in force: a pending engine request, else the last listing's. */
	const decision = $derived(
		conv?.skillsTrust ? 'pending' : (conv?.skillsListing?.listing.project_decision ?? null)
	);
	const trustRequest = $derived(conv?.skillsTrust ?? null);

	function list(page: number, text: string) {
		void request.send({ type: 'list_skills', query: text, page }, skillsSlot);
	}

	function onSearch(event: SubmitEvent) {
		event.preventDefault();
		query = draft.trim();
		list(0, query);
	}

	function reload() {
		draft = '';
		query = '';
		void request.send({ type: 'reload_skills' }, skillsSlot);
	}

	/** A person's decision, never automatic. The listing is asked again after it. */
	async function decide(allow: boolean) {
		await trust.send({ type: 'trust_project_skills', allow }, () => null);
		list(0, query);
	}

	onMount(() => list(0, ''));
</script>

<div class="flex flex-col gap-4 text-sm">
	{#if !conv}
		<p class="text-muted-foreground">Aucune conversation ouverte.</p>
	{:else}
		{#if decision !== null}
			<section
				aria-label="Skills du projet"
				class="flex flex-col gap-2 rounded-md border border-border p-3"
			>
				<h3 class="text-xs font-medium text-muted-foreground">Skills du projet</h3>
				{#if decision === 'pending'}
					<p class="text-xs text-pretty">
						En attente de votre décision pour cette session.
					</p>
				{:else if decision === 'granted'}
					<p class="text-xs text-pretty">Autorisés pour cette session.</p>
				{:else}
					<p class="text-xs text-pretty">Refusés pour cette session.</p>
				{/if}

				{#if trustRequest}
					<p class="font-mono text-xs break-all text-muted-foreground">{trustRequest.project}</p>
					<ul class="flex flex-col gap-0.5 text-xs">
						{#each trustRequest.sources as source, i (i)}
							<li>
								<span class="font-mono">{scopeLabel(source.source)}</span>
								· {plural(source.skills, 'skill', 'skills')}
								· <span class="font-mono break-all text-muted-foreground">{source.path}</span>
							</li>
						{/each}
					</ul>
				{/if}

				<div class="flex flex-wrap gap-2">
					{#if decision === 'pending' || decision === 'denied'}
						<Button size="sm" disabled={trust.pending} onclick={() => decide(true)}>
							Autoriser les skills du projet pour cette session
						</Button>
					{/if}
					{#if decision === 'pending' || decision === 'granted'}
						<Button variant="outline" size="sm" disabled={trust.pending} onclick={() => decide(false)}>
							Refuser
						</Button>
					{/if}
				</div>

				{#if trust.refusal}
					<p role="alert" class="rounded-md bg-destructive/10 px-3 py-2 text-xs text-pretty text-destructive">
						Commande refusée : {trust.refusal}
					</p>
				{/if}
			</section>
		{/if}

		<form role="search" class="flex flex-wrap items-end gap-2" onsubmit={onSearch}>
			<label class="flex min-w-0 flex-1 basis-56 flex-col gap-1 text-xs text-muted-foreground">
				Rechercher un skill
				<Input
					type="search"
					bind:value={draft}
					placeholder="Nom ou description"
					class="text-sm text-foreground"
				/>
			</label>
			<Button type="submit" variant="outline" size="sm" disabled={request.pending}>
				<SearchIcon aria-hidden="true" />
				Rechercher
			</Button>
			<Button type="button" variant="outline" size="sm" disabled={request.pending} onclick={reload}>
				<RefreshCwIcon aria-hidden="true" />
				Recharger
			</Button>
		</form>

		<div role="status" aria-live="polite" class="text-xs text-muted-foreground">
			{#if request.pending}
				Chargement…
			{:else if listing}
				{plural(listing.total, 'skill', 'skills')}
			{/if}
		</div>

		{#if request.refusal}
			<p role="alert" class="rounded-md bg-destructive/10 px-3 py-2 text-xs text-pretty text-destructive">
				Commande refusée : {request.refusal}
			</p>
		{/if}

		{#if listing && listing.diagnostics && listing.diagnostics.length > 0}
			<section
				aria-label="Diagnostic des skills"
				class="flex flex-col gap-1 rounded-md bg-warning-soft px-3 py-2 text-xs text-pretty"
			>
				<h3 class="font-medium">Diagnostic</h3>
				<ul class="flex list-disc flex-col gap-0.5 pl-4">
					{#each listing.diagnostics as diagnostic, i (i)}
						<li class="break-all">{diagnostic}</li>
					{/each}
				</ul>
			</section>
		{/if}

		{#if listing}
			{#if listing.catalog_omitted > 0}
				<p class="text-xs text-pretty text-muted-foreground">
					Activables mais hors du catalogue du modèle (limites de taille) : {listing.catalog_omitted}.
				</p>
			{/if}

			{#if listing.entries.length === 0}
				<p class="text-muted-foreground">
					{query ? `Aucun skill ne correspond à « ${query} ».` : 'Aucun skill disponible.'}
				</p>
			{:else}
				<!-- A named, focusable scroll container: keyboard users must be able to scroll
					it (WCAG 2.1.1). The rule only checks the role, so it is suppressed here. -->
				<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
				<div
					role="region"
					aria-label="Skills"
					tabindex="0"
					class="flex max-h-[46vh] flex-col gap-3 overflow-y-auto rounded-sm outline-none focus-visible:ring-2 focus-visible:ring-ring"
				>
					<ul class="flex flex-col gap-2">
						{#each listing.entries as entry, i (i)}
							{@const view = skillStateView(entry.state)}
							<li class="flex min-w-0 flex-col gap-1.5 rounded-md border border-border px-2.5 py-2">
								<div class="flex min-w-0 flex-wrap items-center gap-2">
									<span class="min-w-0 truncate font-mono font-medium">{entry.name}</span>
									<Badge variant="outline">{scopeLabel(entry.source)}</Badge>
									<Badge variant={view.variant}>{view.label}</Badge>
									{#if entry.revision}
										<span class="font-mono text-xs text-muted-foreground">révision {entry.revision}</span>
									{/if}
								</div>
								{#if entry.description}
									<p class="text-xs text-pretty">{entry.description}</p>
								{/if}
								{#if entry.path}
									<p class="font-mono text-xs break-all text-muted-foreground">{entry.path}</p>
								{/if}
								{#if entry.details && entry.details.length > 0}
									<ul class="flex list-disc flex-col gap-0.5 pl-4 text-xs text-pretty">
										{#each entry.details as detail, j (j)}
											<li class="break-all">{detail}</li>
										{/each}
									</ul>
								{/if}
							</li>
						{/each}
					</ul>
				</div>

				<Pager
					page={listing.page}
					pages={pageCount(listing.total, ENGINE_PAGE_SIZE)}
					disabled={request.pending}
					onchange={(page) => list(page, query)}
				/>
			{/if}
		{/if}
	{/if}
</div>
