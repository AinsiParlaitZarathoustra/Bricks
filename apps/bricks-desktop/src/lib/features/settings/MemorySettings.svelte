<script lang="ts">
	// The long-term memory: on or off (from the next run), the global memory and the open
	// project's, viewed and cleared, and the memories of projects removed from the list.
	// Turning it off deletes nothing; viewing and clearing work either way.
	import { t } from '$lib/i18n/i18n.svelte';
	import { onMount } from 'svelte';
	import type { MemoryCounts, MemoryOverview, MemoryScope } from '$lib/ipc/client';
	import * as ipc from '$lib/ipc/client';
	import { app } from '$lib/state/app.svelte';
	import { ui } from '$lib/state/ui.svelte';
	import { Button } from '$lib/components/ui/button';
	import * as ButtonGroup from '$lib/components/ui/button-group';
	import { Switch } from '$lib/components/ui/switch';
	import SettingsGroup from './SettingsGroup.svelte';
	import DeleteDialog from './DeleteDialog.svelte';
	import type { WriteOutcome } from './models';

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
			t('settings.noteCount', { count: c.notes }),
			t('settings.exchangeCount', { count: c.exchanges }),
			t('settings.factCount', { count: c.facts })
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
			title: t('settings.deleteTheGlobalMemory'),
			description: t('settings.everyProjectLosesAccessToThis', { c: total(c) }),
			run: () => ipc.memoryDelete('global', null)
		};
	}

	function askProject(c: MemoryCounts, id: string, name: string) {
		pending = {
			title: t('settings.deleteThisProjectsMemory'),
			description: t('settings.theConversationHistoryIsKept', { name, c: total(c) }),
			run: () => ipc.memoryDelete('project', id)
		};
	}

	function askRetired(id: string, name: string | null) {
		pending = {
			title: t('settings.deleteThisRemovedProjectsMemory'),
			description: t('settings.theConversationHistoryIsKept2', { id: name ?? id }),
			run: () => ipc.memoryDeleteRetired(id)
		};
	}
</script>

<div class="flex flex-col gap-6">
	<SettingsGroup title={t('common.memory')}>
		<label class="flex items-center justify-between gap-3 p-3 text-ui">
			{t('common.memory')}
			<Switch bind:checked={() => app.prefs.memory_enabled, (on) => toggle(on)} disabled={toggling} />
		</label>
		{#if toggleError}
			<p role="alert" class="p-3 text-ui text-pretty text-destructive">
				{t('settings.theSettingWasntSaved', { saveError: toggleError })}
			</p>
		{/if}
	</SettingsGroup>

	{#if loadError}
		<p role="alert" class="text-ui text-pretty text-destructive">{t('settings.couldNotRead', { loadError })}</p>
	{/if}

	<SettingsGroup title={t('settings.globalMemory')}>
		<div class="flex flex-wrap items-center gap-2 p-3">
			<span class="text-ui tabular-nums text-muted-foreground">
				{overview ? total(overview.global) : '—'}
			</span>
			<ButtonGroup.Root class="ml-auto">
				<Button variant="outline" size="sm" onclick={() => view('global')}>{t('settings.view')}</Button>
				<Button
					variant="outline"
					size="sm"
					class="text-destructive"
					disabled={!overview}
					onclick={() => overview && askGlobal(overview.global)}
				>
					{t('common.delete')}
				</Button>
			</ButtonGroup.Root>
		</div>
	</SettingsGroup>

	<SettingsGroup title={t('settings.memoryPerProject')}>
		<div class="flex flex-wrap items-center gap-2 p-3">
			{#if project}
				<span class="text-ui font-medium">{project.name}</span>
				<span class="text-ui tabular-nums text-muted-foreground">
					{overview?.project ? total(overview.project) : '—'}
				</span>
			{:else}
				<span class="text-ui text-muted-foreground">{t('common.noOpenProject')}</span>
			{/if}
			<ButtonGroup.Root class="ml-auto">
				<Button variant="outline" size="sm" disabled={!project} onclick={() => view('project')}>{t('settings.view')}</Button>
				<Button
					variant="outline"
					size="sm"
					class="text-destructive"
					disabled={!project || !overview?.project}
					onclick={() =>
						project && overview?.project && askProject(overview.project, project.id, project.name)}
				>
					{t('common.delete')}
				</Button>
			</ButtonGroup.Root>
		</div>
		{#if overview?.toml_section_ignored}
			<p class="p-3 text-caption text-muted-foreground">
				{t('settings.theMemorySectionOfBricksToml')}
			</p>
		{/if}
	</SettingsGroup>

	{#if overview && overview.retired.length > 0}
		<SettingsGroup title={t('settings.memoriesOfRemovedProjects')}>
			{#each overview.retired as r (r.project_id)}
				<div class="flex items-center gap-2 p-3">
					<span class="min-w-0 truncate text-ui">{r.name ?? t('settings.unnamedProject')}</span>
					<span class="min-w-0 truncate font-mono text-caption text-muted-foreground">{r.project_id}</span>
					<Button
						variant="ghost"
						size="sm"
						class="ml-auto text-destructive"
						onclick={() => askRetired(r.project_id, r.name)}
					>
						{t('common.delete')}
					</Button>
				</div>
			{/each}
		</SettingsGroup>
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
