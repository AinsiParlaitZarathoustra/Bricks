<script lang="ts">
	// « Skills » of the settings: the skills of the personal and built-in sources, and those of the
	// current project when it is chosen. Reading grants no trust: a project's skills stay pending
	// until the conversation decides. No import, creation, edition or deletion here.
	import { onDestroy, untrack } from 'svelte';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Label } from '$lib/components/ui/label';
	import * as Select from '$lib/components/ui/select';
	import { app } from '$lib/state/app.svelte';
	import { errorMessage, skillsCatalog, type SkillsCatalog } from '$lib/ipc/client';
	import ChevronLeft from '@lucide/svelte/icons/chevron-left';
	import ChevronRight from '@lucide/svelte/icons/chevron-right';
	import RefreshCw from '@lucide/svelte/icons/refresh-cw';
	import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
	import SettingsGroup from './SettingsGroup.svelte';
	import {
		clampPage,
		pageCount,
		skillCountLabel,
		skillSourceLabel,
		skillStateLabel,
		skillStateTone,
		type SkillTone
	} from './skills';

	const uid = $props.id();
	const project = $derived(app.currentProject);

	/** Personal and built-in sources only, or also the current project's (when one is open). */
	let scope = $state<'global' | 'project'>(untrack(() => (app.currentProject ? 'project' : 'global')));
	const projectId = $derived(scope === 'project' && project ? project.id : null);

	let query = $state('');
	/** The search the listing uses: trimmed, and applied a moment after typing. */
	let applied = $state('');
	let page = $state(0);
	let catalog = $state<SkillsCatalog | null>(null);
	let loading = $state(false);
	let error = $state<string | null>(null);

	/** Only the latest request may write: an older answer that arrives late is dropped. */
	let latest = 0;

	async function fetchCatalog(request: { projectId: string | null; query: string; page: number }, reload: boolean) {
		const token = ++latest;
		loading = true;
		try {
			const next = await skillsCatalog(request.projectId, request.query, request.page, reload);
			if (token !== latest) return;
			if (next.listing.entries.length === 0 && request.page > 0) {
				// The listing shrank under this page (after a reload, say): show its last page instead.
				page = clampPage(request.page, next.listing.total);
				return;
			}
			catalog = next;
			error = null;
		} catch (e) {
			if (token !== latest) return;
			// No stale listing under an error: the page says what failed and nothing else.
			catalog = null;
			error = errorMessage(e);
		} finally {
			if (token === latest) loading = false;
		}
	}

	// Lists again when the source, the applied search or the page changes.
	$effect(() => {
		void fetchCatalog({ projectId, query: applied, page }, false);
	});

	let searchTimer: ReturnType<typeof setTimeout> | undefined;

	function onSearchInput(text: string) {
		query = text;
		clearTimeout(searchTimer);
		searchTimer = setTimeout(() => {
			applied = text.trim();
			page = 0;
		}, 250);
	}

	onDestroy(() => clearTimeout(searchTimer));

	function chooseScope(next: string | undefined) {
		if (next !== 'global' && next !== 'project') return;
		scope = next;
		page = 0;
	}

	function reload() {
		void fetchCatalog({ projectId, query: applied, page }, true);
	}

	const listing = $derived(catalog?.listing ?? null);
	const total = $derived(listing?.total ?? 0);
	const pages = $derived(pageCount(total));
	const entries = $derived(listing?.entries ?? []);
	const diagnostics = $derived(listing?.diagnostics ?? []);
	const omitted = $derived(listing?.catalog_omitted ?? 0);
	const scopeLabel = $derived(projectId !== null && project ? `+ projet ${project.name}` : 'Personnelles et intégrées');

	function badgeClass(tone: SkillTone): string {
		if (tone === 'warn') return 'border-warning text-warning';
		if (tone === 'muted') return 'text-muted-foreground';
		return '';
	}
</script>

<div class="flex flex-col gap-6">
	<section class="flex flex-col gap-3" aria-labelledby={`${uid}-title`}>
		<h3 id={`${uid}-title`} class="text-sm font-semibold text-foreground">Skills</h3>

		<div class="flex flex-col gap-3 sm:flex-row sm:items-end">
			<div class="flex min-w-0 flex-col gap-1.5 sm:w-64 sm:shrink-0">
				<Label for={`${uid}-scope`}>Sources</Label>
				<Select.Root type="single" value={scope} onValueChange={chooseScope}>
					<Select.Trigger id={`${uid}-scope`} class="w-full">
						<span class="truncate">{scopeLabel}</span>
					</Select.Trigger>
					<Select.Content>
						<Select.Item value="global" label="Personnelles et intégrées">Personnelles et intégrées</Select.Item>
						{#if project}
							<Select.Item value="project" label={`+ projet ${project.name}`}>+ projet {project.name}</Select.Item>
						{/if}
					</Select.Content>
				</Select.Root>
			</div>

			<div class="flex min-w-0 flex-1 flex-col gap-1.5">
				<Label for={`${uid}-search`}>Rechercher</Label>
				<Input
					id={`${uid}-search`}
					type="search"
					value={query}
					oninput={(event) => onSearchInput(event.currentTarget.value)}
					placeholder="Nom ou description"
					autocomplete="off"
					spellcheck={false}
				/>
			</div>

			<Button variant="outline" size="sm" class="shrink-0" onclick={reload} disabled={loading}>
				<RefreshCw class={loading ? 'animate-spin' : ''} aria-hidden="true" />
				Recharger
			</Button>
		</div>
	</section>

	{#if error}
		<div role="alert" class="flex flex-col items-start gap-3 rounded-lg border border-destructive/40 bg-destructive/5 p-3 text-sm">
			<p class="flex items-start gap-2 text-pretty">
				<TriangleAlert class="mt-0.5 size-4 shrink-0 text-destructive" aria-hidden="true" />
				Impossible de lire les skills : {error}
			</p>
			<Button variant="outline" size="sm" onclick={reload} disabled={loading}>
				<RefreshCw aria-hidden="true" />
				Réessayer
			</Button>
		</div>
	{:else if !catalog}
		<p role="status" class="text-sm text-muted-foreground">Lecture des skills…</p>
	{:else}
		<div class="flex flex-wrap items-center justify-between gap-3 text-sm">
			<span class="tabular-nums text-muted-foreground" aria-live="polite">
				{skillCountLabel(total)}{applied ? ` pour « ${applied} »` : ''}
			</span>
			{#if pages > 1}
				<nav aria-label="Pages des skills" class="flex items-center gap-2">
					<Button variant="ghost" size="sm" disabled={loading || page === 0} onclick={() => (page -= 1)}>
						<ChevronLeft aria-hidden="true" />
						Précédent
					</Button>
					<span class="tabular-nums text-muted-foreground">Page {page + 1} sur {pages}</span>
					<Button variant="ghost" size="sm" disabled={loading || page + 1 >= pages} onclick={() => (page += 1)}>
						Suivant
						<ChevronRight aria-hidden="true" />
					</Button>
				</nav>
			{/if}
		</div>

		{#if entries.length === 0}
			<p class="text-sm text-muted-foreground">
				{applied ? `Aucun skill ne correspond à « ${applied} ».` : 'Aucun skill dans ces sources.'}
			</p>
		{:else}
			<ul
				class="flex flex-col divide-y divide-border rounded-lg border border-border"
				aria-label="Skills"
				aria-busy={loading}
			>
				{#each entries as entry, index (`${index}:${entry.source}:${entry.name}`)}
					{@const tone = skillStateTone(entry.state)}
					{@const details = entry.details ?? []}
					<li class="flex flex-col gap-1.5 p-3">
						<div class="flex flex-wrap items-center gap-2">
							<span class="min-w-0 font-mono text-sm break-all text-foreground">{entry.name}</span>
							<Badge variant={tone === 'ok' ? 'secondary' : 'outline'} class={badgeClass(tone)}>
								{skillStateLabel(entry.state)}
							</Badge>
						</div>
						<p class="text-xs text-muted-foreground">{skillSourceLabel(entry.source)}</p>
						{#if entry.description}
							<p class="text-sm text-pretty text-muted-foreground">{entry.description}</p>
						{/if}
						{#if details.length > 0}
							<details class="text-xs text-muted-foreground">
								<summary class="cursor-pointer hover:text-foreground">Détails ({details.length})</summary>
								<ul class="mt-1 flex list-disc flex-col gap-0.5 pl-4">
									{#each details as detail, i (i)}
										<li class="break-words">{detail}</li>
									{/each}
								</ul>
							</details>
						{/if}
					</li>
				{/each}
			</ul>
		{/if}

		{#if catalog.sources.length > 0}
			<SettingsGroup title="Sources lues">
				<ul class="flex flex-col">
					{#each catalog.sources as source, index (`${index}:${source.source}:${source.path}`)}
						<li class="flex items-start justify-between gap-3 p-3 text-sm">
							<div class="flex min-w-0 flex-col gap-0.5">
								<span class="text-foreground">{skillSourceLabel(source.source)}</span>
								<span class="truncate font-mono text-xs text-muted-foreground" title={source.path}>{source.path}</span>
							</div>
							<span class="shrink-0 tabular-nums text-muted-foreground">{skillCountLabel(source.skills)}</span>
						</li>
					{/each}
				</ul>
			</SettingsGroup>
		{/if}

		{#if diagnostics.length > 0 || omitted > 0}
			<details class="text-sm">
				<summary class="cursor-pointer text-muted-foreground hover:text-foreground">
					Diagnostics ({diagnostics.length + (omitted > 0 ? 1 : 0)})
				</summary>
				<ul class="mt-2 flex list-disc flex-col gap-1 pl-5 text-xs text-muted-foreground">
					{#each diagnostics as line, i (i)}
						<li class="break-words">{line}</li>
					{/each}
					{#if omitted > 0}
						<li>{omitted} skill(s) activable(s) ne figurent pas dans le catalogue du modèle, à cause des limites du moteur.</li>
					{/if}
				</ul>
			</details>
		{/if}
	{/if}
</div>
