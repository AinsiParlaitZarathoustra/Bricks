<script lang="ts">
	// « Skills » of the settings: the built-in skill and the skills installed in Bricks' store, with
	// the three actions on the store — write a new one, import a folder, remove an installed one.
	// Every change goes through the engine's store (validated, published at once, nothing restarted);
	// open conversations see it at their next request.
	import { onDestroy } from 'svelte';
	import { open as openDialog } from '@tauri-apps/plugin-dialog';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Label } from '$lib/components/ui/label';
	import { Textarea } from '$lib/components/ui/textarea';
	import {
		errorMessage,
		skillsCatalog,
		skillsCreate,
		skillsImport,
		skillsRemove,
		type SkillChange,
		type SkillsCatalog
	} from '$lib/ipc/client';
	import ChevronLeft from '@lucide/svelte/icons/chevron-left';
	import ChevronRight from '@lucide/svelte/icons/chevron-right';
	import FolderInput from '@lucide/svelte/icons/folder-input';
	import Plus from '@lucide/svelte/icons/plus';
	import RefreshCw from '@lucide/svelte/icons/refresh-cw';
	import Trash2 from '@lucide/svelte/icons/trash-2';
	import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
	import SettingsGroup from './SettingsGroup.svelte';
	import {
		clampPage,
		isInstalledSkill,
		pageCount,
		skillCountLabel,
		skillSourceLabel,
		skillStateLabel,
		skillStateTone,
		type SkillTone
	} from './skills';

	const uid = $props.id();

	let query = $state('');
	/** The search the listing uses: trimmed, and applied a moment after typing. */
	let applied = $state('');
	let page = $state(0);
	let catalog = $state.raw<SkillsCatalog | null>(null);
	let loading = $state(false);
	let error = $state<string | null>(null);

	/** Only the latest request may write: an older answer that arrives late is dropped. */
	let latest = 0;

	async function fetchCatalog(request: { query: string; page: number }, reload: boolean) {
		const token = ++latest;
		loading = true;
		try {
			const next = await skillsCatalog(null, request.query, request.page, reload);
			if (token !== latest) return;
			if (next.listing.entries.length === 0 && request.page > 0) {
				// The listing shrank under this page (after a removal, say): show its last page instead.
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

	// Lists again when the applied search or the page changes.
	$effect(() => {
		void fetchCatalog({ query: applied, page }, false);
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

	function reload() {
		void fetchCatalog({ query: applied, page }, true);
	}

	// ─── Changes to the store ─────────────────────────────────────────────────

	/** One change at a time: the buttons wait for the answer. */
	let busy = $state(false);
	/** The last refusal of an import or a removal (the form shows its own under its fields). */
	let problem = $state<string | null>(null);
	/** What the last change did, read by screen readers and shown next to the count. */
	let done = $state<string | null>(null);

	let adding = $state(false);
	let name = $state('');
	let description = $state('');
	let instructions = $state('');
	/** The refused field and why; the entered text stays. */
	let formError = $state<{ field: string; message: string } | null>(null);

	let confirming = $state<string | null>(null);

	const instructionsPlaceholder = '# Titre\n\n1. Première étape\n2. …';

	async function change(run: () => Promise<SkillChange>, success: (name: string) => string): Promise<SkillChange | null> {
		busy = true;
		problem = null;
		done = null;
		try {
			const result = await run();
			if (result.name) {
				done = success(result.name);
				await fetchCatalog({ query: applied, page }, false);
			}
			return result;
		} catch (e) {
			problem = errorMessage(e);
			return null;
		} finally {
			busy = false;
		}
	}

	function startAdding() {
		adding = true;
		confirming = null;
		problem = null;
	}

	function cancelAdding() {
		adding = false;
		formError = null;
	}

	async function submitNew(event: SubmitEvent) {
		event.preventDefault();
		formError = null;
		const result = await change(
			() => skillsCreate(name.trim(), description, instructions),
			(n) => `« ${n} » ajouté`
		);
		if (!result) return;
		if (result.problem) {
			formError = { field: result.problem.field, message: result.problem.message };
			return;
		}
		adding = false;
		name = '';
		description = '';
		instructions = '';
	}

	async function importFolder() {
		const picked = await openDialog({ directory: true, multiple: false, title: 'Dossier du skill à importer' });
		if (!picked || Array.isArray(picked)) return;
		const result = await change(() => skillsImport(picked), (n) => `« ${n} » importé`);
		if (result?.problem) problem = result.problem.message;
	}

	async function remove(skill: string) {
		confirming = null;
		const result = await change(() => skillsRemove(skill), (n) => `« ${n} » supprimé`);
		if (result?.problem) problem = result.problem.message;
	}

	function fieldError(field: string): string | null {
		return formError?.field === field ? formError.message : null;
	}

	/** Errors about the document as a whole are shown under the form, not under one field. */
	const otherFormError = $derived(
		formError && !['name', 'description', 'instructions'].includes(formError.field) ? formError.message : null
	);

	const listing = $derived(catalog?.listing ?? null);
	const total = $derived(listing?.total ?? 0);
	const pages = $derived(pageCount(total));
	const entries = $derived(listing?.entries ?? []);
	const diagnostics = $derived(listing?.diagnostics ?? []);
	const omitted = $derived(listing?.catalog_omitted ?? 0);

	function badgeClass(tone: SkillTone): string {
		if (tone === 'warn') return 'border-warning text-warning';
		if (tone === 'muted') return 'text-muted-foreground';
		return '';
	}
</script>

{#snippet fieldMessage(field: string)}
	{@const message = fieldError(field)}
	{#if message}
		<p id={`${uid}-${field}-error`} class="text-xs text-pretty text-destructive">{message}</p>
	{/if}
{/snippet}

<div class="flex flex-col gap-6">
	<section class="flex flex-col gap-3" aria-labelledby={`${uid}-title`}>
		<h3 id={`${uid}-title`} class="text-ui font-semibold text-foreground">Skills</h3>

		<div class="flex flex-col gap-3 sm:flex-row sm:items-end">
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

			<div class="flex shrink-0 flex-wrap gap-2">
				<Button variant="outline" size="sm" onclick={startAdding} disabled={busy || adding}>
					<Plus aria-hidden="true" />
					Ajouter
				</Button>
				<Button variant="outline" size="sm" onclick={importFolder} disabled={busy}>
					<FolderInput aria-hidden="true" />
					Importer
				</Button>
				<Button variant="outline" size="sm" onclick={reload} disabled={loading}>
					<RefreshCw class={loading ? 'animate-spin' : ''} aria-hidden="true" />
					Recharger
				</Button>
			</div>
		</div>
	</section>

	{#if adding}
		<form
			class="flex flex-col gap-3 rounded-lg border border-border p-3"
			aria-label="Nouveau skill"
			onsubmit={submitNew}
		>
			<div class="flex flex-col gap-1.5">
				<Label for={`${uid}-name`}>Nom</Label>
				<Input
					id={`${uid}-name`}
					bind:value={name}
					class="font-mono"
					placeholder="release-notes"
					autocomplete="off"
					spellcheck={false}
					required
					aria-invalid={fieldError('name') ? true : undefined}
					aria-describedby={fieldError('name') ? `${uid}-name-error` : undefined}
				/>
				{@render fieldMessage('name')}
			</div>
			<div class="flex flex-col gap-1.5">
				<Label for={`${uid}-description`}>Description</Label>
				<Input
					id={`${uid}-description`}
					bind:value={description}
					placeholder="Ce que fait le skill et quand l'utiliser"
					autocomplete="off"
					required
					aria-invalid={fieldError('description') ? true : undefined}
					aria-describedby={fieldError('description') ? `${uid}-description-error` : undefined}
				/>
				{@render fieldMessage('description')}
			</div>
			<div class="flex flex-col gap-1.5">
				<Label for={`${uid}-instructions`}>Instructions</Label>
				<Textarea
					id={`${uid}-instructions`}
					bind:value={instructions}
					rows={8}
					class="font-mono text-xs"
					placeholder={instructionsPlaceholder}
					spellcheck={false}
					required
					aria-invalid={fieldError('instructions') ? true : undefined}
					aria-describedby={fieldError('instructions') ? `${uid}-instructions-error` : undefined}
				/>
				{@render fieldMessage('instructions')}
			</div>
			{#if otherFormError}
				<p role="alert" class="flex items-start gap-2 text-xs text-pretty text-destructive">
					<TriangleAlert class="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
					{otherFormError}
				</p>
			{/if}
			<div class="flex justify-end gap-2">
				<Button type="button" variant="ghost" size="sm" onclick={cancelAdding} disabled={busy}>Annuler</Button>
				<Button type="submit" size="sm" disabled={busy}>Ajouter</Button>
			</div>
		</form>
	{/if}

	{#if problem}
		<div role="alert" class="flex items-start justify-between gap-3 rounded-lg border border-destructive/40 bg-destructive/5 p-3 text-ui">
			<p class="flex items-start gap-2 text-pretty">
				<TriangleAlert class="mt-0.5 size-4 shrink-0 text-destructive" aria-hidden="true" />
				{problem}
			</p>
			<Button variant="ghost" size="sm" class="shrink-0" onclick={() => (problem = null)}>Fermer</Button>
		</div>
	{/if}

	{#if error}
		<div role="alert" class="flex flex-col items-start gap-3 rounded-lg border border-destructive/40 bg-destructive/5 p-3 text-ui">
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
		<p role="status" class="text-ui text-muted-foreground">Lecture des skills…</p>
	{:else}
		<div class="flex flex-wrap items-center justify-between gap-3 text-ui">
			<span class="tabular-nums text-muted-foreground" aria-live="polite">
				{skillCountLabel(total)}{applied ? ` pour « ${applied} »` : ''}{done ? ` · ${done}` : ''}
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
			<p class="text-ui text-muted-foreground">
				{applied ? `Aucun skill ne correspond à « ${applied} ».` : 'Aucun skill.'}
			</p>
		{:else}
			<ul
				class="flex flex-col divide-y divide-border rounded-lg border border-border"
				aria-label="Skills"
				aria-busy={loading}
			>
				{#each entries as entry (`${entry.source}:${entry.name}`)}
					{@const tone = skillStateTone(entry.state)}
					{@const details = entry.details ?? []}
					{@const installed = isInstalledSkill(entry.source)}
					<li class="flex flex-col gap-1.5 p-3">
						<div class="flex flex-wrap items-center gap-2">
							<span class="min-w-0 font-mono text-ui break-all text-foreground">{entry.name}</span>
							<Badge variant={tone === 'ok' ? 'secondary' : 'outline'} class={badgeClass(tone)}>
								{skillStateLabel(entry.state)}
							</Badge>
							{#if installed}
								<div class="ml-auto flex items-center gap-1">
									{#if confirming === entry.name}
										<Button variant="destructive" size="sm" onclick={() => remove(entry.name)} disabled={busy}>
											Supprimer
										</Button>
										<Button variant="ghost" size="sm" onclick={() => (confirming = null)} disabled={busy}>Annuler</Button>
									{:else}
										<Button
											variant="ghost"
											size="icon-sm"
											aria-label={`Supprimer ${entry.name}`}
											title="Supprimer"
											onclick={() => (confirming = entry.name)}
											disabled={busy}
										>
											<Trash2 aria-hidden="true" />
										</Button>
									{/if}
								</div>
							{/if}
						</div>
						<p class="text-xs text-muted-foreground">{skillSourceLabel(entry.source)}</p>
						{#if entry.description}
							<p class="text-xs text-pretty text-muted-foreground">{entry.description}</p>
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
					{#each catalog.sources as source (`${source.source}:${source.path}`)}
						<li class="flex items-start justify-between gap-3 p-3 text-ui">
							<div class="flex min-w-0 flex-col gap-0.5">
								<span class="text-foreground">{skillSourceLabel(source.source)}</span>
								{#if source.path}
									<span class="truncate font-mono text-xs text-muted-foreground" title={source.path}>{source.path}</span>
								{/if}
							</div>
							<span class="shrink-0 tabular-nums text-muted-foreground">{skillCountLabel(source.skills)}</span>
						</li>
					{/each}
				</ul>
			</SettingsGroup>
		{/if}

		{#if diagnostics.length > 0 || omitted > 0}
			<details class="text-ui">
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
