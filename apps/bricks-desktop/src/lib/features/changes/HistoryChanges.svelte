<script lang="ts">
	import { t } from '$lib/i18n/i18n.svelte';
	import { untrack } from 'svelte';
	import { changeLogDiff, type ChangeRecord } from '$lib/ipc/client';
	import { Button } from '$lib/components/ui/button';
	import EmptyState from '$lib/layout/EmptyState.svelte';
	import { app } from '$lib/state/app.svelte';
	import { ui } from '$lib/state/ui.svelte';
	import { changesetIdsOf, changesetLinkLabel } from '$lib/features/catalog/changeset-links';
	import FileChangeRow from './FileChangeRow.svelte';
	import ChangeTree from './ChangeTree.svelte';
	import { buildFileTree } from './tree';
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
		return session?.title.trim() || t('changes.conversation', { sessionId: shortId(sessionId) });
	}

	function patchesOf(record: ChangeRecord, path: string, hasPatch: boolean): PatchSource[] {
		if (!hasPatch) return [];
		return [
			{
				key: `${record.id}:${path}`,
				title: t('changes.diffOfTheOperationOf', { at: formatDateTime(record.at) }),
				load: () => changeLogDiff(record.id, path),
			},
		];
	}
</script>

{#if shown.view === 'no_project'}
	<EmptyState title={t('common.noOpenProject')} description={t('changes.chooseAProjectToSeeIts2')} />
{:else if shown.view === 'failed' && journal}
	<EmptyState title={t('changes.historyUnavailable2')} description={journal.error ?? undefined}>
		{#snippet action()}
			<Button variant="outline" size="sm" onclick={() => journal?.load()}>{t('common.retry')}</Button>
		{/snippet}
	</EmptyState>
{:else if shown.view === 'loading'}
	<p class="px-3 py-6 text-center text-sm text-muted-foreground">{t('common.loading')}</p>
{:else if shown.view === 'entries' && journal}
	<div class="flex flex-col gap-3">
		{#if shown.inlineError}
			<div role="alert" class="flex flex-wrap items-center justify-between gap-2 text-xs text-destructive">
				<p class="min-w-0 break-words">{t('changes.couldNotLoadMore', { inlineError: shown.inlineError })}</p>
				<Button variant="outline" size="sm" disabled={journal.loading} onclick={() => journal?.more()}>{t('common.retry')}</Button>
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
							{record.tool}{record.agent_id ? t('changes.agent', { agentId: shortId(record.agent_id) }) : ''}{record.changeset_id
								? t('changes.changeset', { changesetId: shortId(record.changeset_id) })
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
				<div class="mt-1 border-t border-border p-1">
					<ChangeTree nodes={buildFileTree(record.files, (entry) => entry.path)}>
					{#snippet file(entry, name)}
						<FileChangeRow
							name={name}
							path={entry.path}
							status={kindLabel(entry.kind)}
							added={entry.added}
							removed={entry.removed}
							binary={entry.binary}
							patches={patchesOf(record, entry.path, entry.has_patch)}
						/>
					{/snippet}
					</ChangeTree>
				</div>
			</article>
		{/each}

		{#if !journal.exhausted}
			<div class="flex justify-center">
				<Button variant="outline" size="sm" disabled={journal.loading} onclick={() => journal?.more()}>
					{journal.loading ? t('common.loading') : t('changes.loadMore')}
				</Button>
			</div>
		{/if}
	</div>
{/if}
