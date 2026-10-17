<script lang="ts">
	import { untrack } from 'svelte';
	import { changeLogDiff } from '$lib/ipc/client';
	import { Button } from '$lib/components/ui/button';
	import EmptyState from '$lib/layout/EmptyState.svelte';
		import { app } from '$lib/state/app.svelte';
	import { ui } from '$lib/state/ui.svelte';
	import { changesetIdsOf, changesetLinkLabel } from '$lib/features/catalog/changeset-links';
	import { conversationChanges, type FileView } from './conversation-changes';
	import FileChangeRow from './FileChangeRow.svelte';
	import { sessionJournal, type SessionJournal } from './journal.svelte';
	import { formatDateTime, kindLabel, plural, shortId } from './format';
	import type { PatchSource } from './types';

	const conv = $derived(app.current);
	const journal = $derived.by<SessionJournal | null>(() => {
		const projectId = conv?.projectId;
		return conv && projectId ? sessionJournal(projectId, conv.sessionId) : null;
	});

	// Load once per conversation. An error waits for « Réessayer », no retry loop.
	$effect(() => {
		const current = journal;
		if (current && !current.loaded && !current.loading && !current.error) {
			untrack(() => void current.load());
		}
	});

	const groups = $derived(
		conv
			? conversationChanges(conv.items, journal?.records ?? [], (runId) => conv.runs.get(runId)?.startedAt)
			: []
	);

	const fileCount = $derived(new Set(groups.flatMap((g) => g.files.map((f) => f.path))).size);

	function promptOf(runId: string | null): string {
		const prompt = runId && conv ? conv.runs.get(runId)?.prompt.trim() : '';
		if (!prompt) return 'Exécution';
		return prompt.length > 80 ? `${prompt.slice(0, 80)}…` : prompt;
	}

	/** One patch per journal record that holds this file. Live edits have none yet. */
	function patchesOf(file: FileView): PatchSource[] {
		return file.operations.flatMap((op) => {
			const recordId = op.recordId;
			if (!recordId || !op.hasPatch) return [];
			return [
				{
					key: `${recordId}:${file.path}`,
					title: op.at ? `Opération du ${formatDateTime(op.at)}` : 'Opération',
					load: () => changeLogDiff(recordId, file.path),
				},
			];
		});
	}
</script>

{#if !conv}
	<EmptyState
		title="Aucune conversation ouverte"
		description="Ouvrez une conversation pour voir les fichiers qu’elle a modifiés."
	/>
{:else if groups.length === 0}
	{#if journal?.error}
		<EmptyState title="Journal indisponible" description={journal.error}>
			{#snippet action()}
				<Button variant="outline" size="sm" onclick={() => journal?.load()}>Réessayer</Button>
			{/snippet}
		</EmptyState>
		{:else if !journal?.loaded}
		<p class="px-3 py-6 text-center text-sm text-muted-foreground">Chargement…</p>
	{/if}
{:else}
	<div class="flex flex-col gap-4">
				<p class="text-xs text-muted-foreground">
			{plural(fileCount, 'fichier modifié', 'fichiers modifiés')}
		</p>

		{#each groups as group (group.runId ?? 'sans-execution')}
			<section class="flex flex-col gap-1" aria-label={promptOf(group.runId)}>
				<header class="flex items-baseline justify-between gap-2 text-xs text-muted-foreground">
					<h3 class="min-w-0 truncate font-medium text-foreground">{promptOf(group.runId)}</h3>
					{#if group.startedAt}
						<time class="shrink-0 tabular-nums">{formatDateTime(group.startedAt)}</time>
					{/if}
				</header>
				<div class="rounded-lg border border-border bg-card px-3">
					{#each group.files as file (file.path)}
						<FileChangeRow
							path={file.path}
							status={kindLabel(file.kind)}
							added={file.added}
							removed={file.removed}
							binary={file.binary}
							agents={file.agents.map((id) => `Agent ${shortId(id)}`)}
							patches={patchesOf(file)}
						>
							{#snippet actions()}
								{@const ids = changesetIdsOf(file.operations)}
								{#each ids as id (id)}
									<Button
										variant="ghost"
										size="xs"
										onclick={() => ui.open('changeset', { changesetId: id })}
									>
										{changesetLinkLabel(ids, id)}
									</Button>
								{/each}
							{/snippet}
						</FileChangeRow>
					{/each}
				</div>
			</section>
		{/each}
	</div>
{/if}
