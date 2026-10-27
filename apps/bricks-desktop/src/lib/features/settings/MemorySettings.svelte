<script lang="ts">
	// The long-term memory: on or off (from the next run), the global memory and the open
	// project's, viewed and cleared, and the memories of projects removed from the list.
	// Turning it off deletes nothing; viewing and clearing work either way.
	import { onMount } from 'svelte';
	import type { MemoryCounts, MemoryOverview, MemoryScope } from '$lib/ipc/client';
	import * as ipc from '$lib/ipc/client';
	import { app } from '$lib/state/app.svelte';
	import { ui } from '$lib/state/ui.svelte';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import SettingsGroup from './SettingsGroup.svelte';
	import DeleteDialog from './DeleteDialog.svelte';
	import type { WriteOutcome } from './models';
	import { plural } from '$lib/features/inspect/format';

	const project = $derived(app.currentProject && !app.currentProject.chats ? app.currentProject : null);

	let overview = $state<MemoryOverview | null>(null);
	let loadError = $state<string | null>(null);
	let toggleError = $state<string | null>(null);
	let toggling = $state(false);

	/** The deletion waiting for its confirmation. */
	let pending = $state<{ title: string; description: string; run: () => Promise<void> } | null>(
		null
	);

	async function load() {
		try {
			overview = await ipc.memoryOverview(project?.id ?? null);
			loadError = null;
		} catch (e) {
			loadError = ipc.errorMessage(e);
		}
	}

	onMount(() => {
		void load();
	});

	async function toggle(on: boolean) {
		toggling = true;
		toggleError = await app.setMemoryEnabled(on);
		toggling = false;
	}

	function total(c: MemoryCounts): string {
		return [
			plural(c.notes, 'note', 'notes'),
			plural(c.exchanges, 'échange', 'échanges'),
			plural(c.facts, 'fait', 'faits')
		].join(' · ');
	}

	function view(scope: MemoryScope) {
		ui.open('memory', { scope });
	}

	async function confirmed(run: () => Promise<void>): Promise<WriteOutcome> {
		try {
			await run();
			await load();
			return { ok: true };
		} catch (e) {
			return { ok: false, message: ipc.errorMessage(e), conflict: false };
		}
	}

	function askGlobal(c: MemoryCounts) {
		pending = {
			title: 'Supprimer la mémoire globale',
			description: `${total(c)}. Tous les projets perdent l’accès à cette mémoire partagée. L’historique des conversations est conservé.`,
			run: () => ipc.memoryDelete('global', null)
		};
	}

	function askProject(c: MemoryCounts, id: string, name: string) {
		pending = {
			title: 'Supprimer la mémoire de ce projet',
			description: `${name} : ${total(c)}. L’historique des conversations est conservé.`,
			run: () => ipc.memoryDelete('project', id)
		};
	}

	function askRetired(id: string, name: string | null) {
		pending = {
			title: 'Supprimer la mémoire de ce projet retiré',
			description: `${name ?? id}. L’historique des conversations est conservé.`,
			run: () => ipc.memoryDeleteRetired(id)
		};
	}
</script>

<div class="flex flex-col gap-6">
	<SettingsGroup title="Mémoire">
		<div class="flex flex-wrap items-center gap-3 p-3">
			{#if app.prefs.memory_enabled}
				<Badge>Mémoire activée</Badge>
				<Button variant="secondary" size="sm" class="ml-auto" disabled={toggling} onclick={() => toggle(false)}>
					Désactiver
				</Button>
			{:else}
				<Button size="sm" disabled={toggling} onclick={() => toggle(true)}>Activer la mémoire</Button>
			{/if}
		</div>
		{#if toggleError}
			<p role="alert" class="p-3 text-ui text-pretty text-destructive">
				Le réglage n’a pas été enregistré : {toggleError}
			</p>
		{/if}
	</SettingsGroup>

	{#if loadError}
		<p role="alert" class="text-ui text-pretty text-destructive">Lecture impossible : {loadError}</p>
	{/if}

	<SettingsGroup title="Mémoire globale">
		<div class="flex flex-wrap items-center gap-2 p-3">
			<span class="text-ui tabular-nums text-muted-foreground">
				{overview ? total(overview.global) : '—'}
			</span>
			<div class="ml-auto flex gap-2">
				<Button variant="secondary" size="sm" onclick={() => view('global')}>Voir</Button>
				<Button
					variant="ghost"
					size="sm"
					class="text-destructive"
					disabled={!overview}
					onclick={() => overview && askGlobal(overview.global)}
				>
					Supprimer
				</Button>
			</div>
		</div>
	</SettingsGroup>

	<SettingsGroup title="Mémoire par projet">
		<div class="flex flex-wrap items-center gap-2 p-3">
			{#if project}
				<span class="text-ui font-medium">{project.name}</span>
				<span class="text-ui tabular-nums text-muted-foreground">
					{overview?.project ? total(overview.project) : '—'}
				</span>
			{:else}
				<span class="text-ui text-muted-foreground">Aucun projet ouvert</span>
			{/if}
			<div class="ml-auto flex gap-2">
				<Button variant="secondary" size="sm" disabled={!project} onclick={() => view('project')}>Voir</Button>
				<Button
					variant="ghost"
					size="sm"
					class="text-destructive"
					disabled={!project || !overview?.project}
					onclick={() =>
						project && overview?.project && askProject(overview.project, project.id, project.name)}
				>
					Supprimer
				</Button>
			</div>
		</div>
		{#if overview?.toml_section_ignored}
			<p class="p-3 text-caption text-muted-foreground">
				La section [memory] de bricks.toml n’est pas utilisée par l’app.
			</p>
		{/if}
	</SettingsGroup>

	{#if overview && overview.retired.length > 0}
		<SettingsGroup title="Mémoires de projets retirés">
			{#each overview.retired as r (r.project_id)}
				<div class="flex items-center gap-2 p-3">
					<span class="min-w-0 truncate text-ui">{r.name ?? 'Projet sans nom'}</span>
					<span class="min-w-0 truncate font-mono text-caption text-muted-foreground">{r.project_id}</span>
					<Button
						variant="ghost"
						size="sm"
						class="ml-auto text-destructive"
						onclick={() => askRetired(r.project_id, r.name)}
					>
						Supprimer
					</Button>
				</div>
			{/each}
		</SettingsGroup>
	{/if}

	{#if overview?.legacy_store}
		<p class="text-caption text-muted-foreground">
			L’ancienne mémoire (memory.grafeo) est conservée telle quelle, sans migration.
		</p>
	{/if}
</div>

{#if pending}
	{@const p = pending}
	<DeleteDialog
		title={p.title}
		description={p.description}
		onConfirm={() => confirmed(p.run)}
		onClose={() => (pending = null)}
	/>
{/if}
