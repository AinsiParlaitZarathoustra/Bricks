<script lang="ts">
	import { untrack } from 'svelte';
	import { changeLogDiff, type ChangeRecord } from '$lib/ipc/client';
	import { Button } from '$lib/components/ui/button';
	import EmptyState from '$lib/layout/EmptyState.svelte';
	import { app } from '$lib/state/app.svelte';
	import { ui } from '$lib/state/ui.svelte';
	import { changesetIdsOf, changesetLinkLabel } from '$lib/features/catalog/changeset-links';
	import FileChangeRow from './FileChangeRow.svelte';
	import { projectJournal, type ProjectJournal } from './journal.svelte';
	import { formatDateTime, kindLabel, shortId } from './format';
	import { historyView } from './history-state';
	import type { PatchSource } from './types';

	const journal = $derived.by<ProjectJournal | null>(() => {
		const projectId = app.currentProjectId;
		return projectId ? projectJournal(projectId) : null;
	});

	const shown = $derived(
		historyView({
			hasProject: journal !== null,
			loaded: journal?.loaded ?? false,
			error: journal?.error ?? null,
			count: journal?.records.length ?? 0
		})
	);

	// Load once per project. An error waits for « Réessayer », no retry loop.
	$effect(() => {
		const current = journal;
		if (current && !current.loaded && !current.loading && !current.error) {
			untrack(() => void current.load());
		}
	});

	function sessionTitle(sessionId: string): string {
		const session = app.sessions.find((s) => s.id === sessionId);
		return session?.title.trim() || `Conversation ${shortId(sessionId)}`;
	}

	function patchesOf(record: ChangeRecord, path: string, hasPatch: boolean): PatchSource[] {
		if (!hasPatch) return [];
		return [
			{
				key: `${record.id}:${path}`,
				title: `Diff de l’opération du ${formatDateTime(record.at)}`,
				load: () => changeLogDiff(record.id, path),
			},
		];
	}
</script>

{#if shown.view === 'no_project'}
	<EmptyState title="Aucun projet ouvert" description="Choisissez un projet pour voir son historique de modifications." />
{:else if shown.view === 'failed' && journal}
	<EmptyState title="Historique indisponible" description={journal.error ?? undefined}>
		{#snippet action()}
			<Button variant="outline" size="sm" onclick={() => journal?.load()}>Réessayer</Button>
		{/snippet}
	</EmptyState>
{:else if shown.view === 'loading'}
	<p class="px-3 py-6 text-center text-sm text-muted-foreground">Chargement…</p>
{:else if shown.view === 'entries' && journal}
	<div class="flex flex-col gap-3">
		{#if shown.inlineError}
			<div role="alert" class="flex flex-wrap items-center justify-between gap-2 text-xs text-destructive">
				<p class="min-w-0 break-words">Impossible de charger la suite : {shown.inlineError}</p>
				<Button variant="outline" size="sm" disabled={journal.loading} onclick={() => journal?.more()}>Réessayer</Button>
			</div>
		{/if}

		{#each journal.records as record (record.id)}
			<article class="rounded-lg border border-border bg-card">
				<header class="flex flex-col gap-0.5 px-3 pt-2">
					<div class="flex items-baseline justify-between gap-2">
						<h3 class="min-w-0 truncate text-sm font-medium">{sessionTitle(record.session_id)}</h3>
						<time class="shrink-0 text-xs tabular-nums text-muted-foreground">{formatDateTime(record.at)}</time>
					</div>
					<div class="flex min-w-0 items-center justify-between gap-2">
						<p class="min-w-0 truncate text-xs text-muted-foreground">
							{record.tool}{record.agent_id ? ` · Agent ${shortId(record.agent_id)}` : ''}{record.changeset_id
								? ` · Changeset ${shortId(record.changeset_id)}`
								: ''}
						</p>
						{#each changesetIdsOf([{ changesetId: record.changeset_id }]) as id (id)}
							<Button
								variant="ghost"
								size="xs"
								class="shrink-0"
								onclick={() => ui.open('changeset', { changesetId: id })}
							>
								{changesetLinkLabel([id], id)}
							</Button>
						{/each}
					</div>
				</header>
				<div class="mt-1 border-t border-border px-3">
					{#each record.files as file (file.path)}
						<FileChangeRow
							path={file.path}
							status={kindLabel(file.kind)}
							added={file.added}
							removed={file.removed}
							binary={file.binary}
							patches={patchesOf(record, file.path, file.has_patch)}
						/>
					{/each}
				</div>
			</article>
		{/each}

		{#if !journal.exhausted}
			<div class="flex justify-center">
				<Button variant="outline" size="sm" disabled={journal.loading} onclick={() => journal?.more()}>
					{journal.loading ? 'Chargement…' : 'Charger plus'}
				</Button>
			</div>
		{/if}
	</div>
{/if}
