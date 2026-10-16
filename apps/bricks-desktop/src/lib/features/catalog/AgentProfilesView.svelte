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
	import { ENGINE_PAGE_SIZE, pageCount, scopeLabel } from './labels';
	import { EngineRequest, profilesSlot, type ProfilesAnswer } from './request.svelte';

	// Opened without options: the view always lists the engine's profiles.
	let { args: _args }: { args: InspectorState['args'] } = $props();

	const conv = $derived(app.current);
	const request = new EngineRequest();
	/** Text of the search box, and the query of the last search sent. */
	let draft = $state('');
	let query = $state('');

	/** The engine's answer to the last request; null while it is awaited. */
	const profiles = $derived(request.answered<ProfilesAnswer>());

	function list(page: number, text: string) {
		void request.send({ type: 'list_agent_profiles', query: text, page }, profilesSlot);
	}

	function onSearch(event: SubmitEvent) {
		event.preventDefault();
		query = draft.trim();
		list(0, query);
	}

	function reload() {
		draft = '';
		query = '';
		void request.send({ type: 'reload_agent_profiles' }, profilesSlot);
	}

	onMount(() => list(0, ''));
</script>

<div class="flex flex-col gap-4 text-sm">
	<p class="text-xs text-pretty text-muted-foreground">
		Profils d’agents du projet, de l’utilisateur et intégrés à Bricks. Un profil masque les
		définitions de priorité inférieure qui portent le même nom.
	</p>

	{#if !conv}
		<p class="text-muted-foreground">Aucune conversation ouverte.</p>
	{:else}
		<form role="search" class="flex flex-wrap items-end gap-2" onsubmit={onSearch}>
			<label class="flex min-w-0 flex-1 basis-56 flex-col gap-1 text-xs text-muted-foreground">
				Rechercher un profil
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
			{:else if profiles}
				{plural(profiles.total, 'profil', 'profils')}
			{/if}
		</div>

		{#if request.refusal}
			<p role="alert" class="rounded-md bg-destructive/10 px-3 py-2 text-xs text-pretty text-destructive">
				Commande refusée : {request.refusal}
			</p>
		{/if}

		{#if profiles && profiles.diagnostics && profiles.diagnostics.length > 0}
			<section
				aria-label="Diagnostic des profils"
				class="flex flex-col gap-1 rounded-md bg-warning-soft px-3 py-2 text-xs text-pretty"
			>
				<h3 class="font-medium">Diagnostic</h3>
				<ul class="flex list-disc flex-col gap-0.5 pl-4">
					{#each profiles.diagnostics as diagnostic, i (i)}
						<li><span class="font-mono break-all">{diagnostic.source}</span> : {diagnostic.message}</li>
					{/each}
				</ul>
			</section>
		{/if}

		{#if profiles}
			{#if profiles.profiles.length === 0}
				<p class="text-muted-foreground">
					{query ? `Aucun profil ne correspond à « ${query} ».` : 'Aucun profil d’agent défini.'}
				</p>
			{:else}
				<!-- A named, focusable scroll container: keyboard users must be able to scroll
					it (WCAG 2.1.1). The rule only checks the role, so it is suppressed here. -->
				<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
				<div
					role="region"
					aria-label="Profils d’agents"
					tabindex="0"
					class="flex max-h-[52vh] flex-col gap-3 overflow-y-auto rounded-sm outline-none focus-visible:ring-2 focus-visible:ring-ring"
				>
					<ul class="flex flex-col gap-2">
						{#each profiles.profiles as profile, i (i)}
							<li class="flex min-w-0 flex-col gap-1.5 rounded-md border border-border px-2.5 py-2">
								<div class="flex min-w-0 flex-wrap items-center gap-2">
									<span class="min-w-0 truncate font-mono font-medium">{profile.name}</span>
									<Badge variant="outline">{scopeLabel(profile.scope)}</Badge>
									{#if profile.valid}
										<Badge variant="secondary">Valide</Badge>
									{:else}
										<Badge variant="destructive">Invalide</Badge>
									{/if}
								</div>
								{#if profile.description}
									<p class="text-xs text-pretty">{profile.description}</p>
								{/if}
								<p class="font-mono text-xs break-all text-muted-foreground">{profile.source}</p>
								{#if profile.error}
									<p class="text-xs text-pretty text-destructive">{profile.error}</p>
								{/if}
								{#if profile.shadows && profile.shadows.length > 0}
									<p class="text-xs text-muted-foreground">
										Masque : <span class="font-mono break-all">{profile.shadows.join(', ')}</span>
									</p>
								{/if}
							</li>
						{/each}
					</ul>
				</div>

				<Pager
					page={profiles.page}
					pages={pageCount(profiles.total, ENGINE_PAGE_SIZE)}
					disabled={request.pending}
					onchange={(page) => list(page, query)}
				/>
			{/if}
		{/if}
	{/if}
</div>
