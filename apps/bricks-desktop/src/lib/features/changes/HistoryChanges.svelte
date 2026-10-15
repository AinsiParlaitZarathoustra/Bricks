<script lang="ts">
	import { untrack } from 'svelte';
	import { changeLogDiff, type ChangeRecord } from '$lib/ipc/client';
	import { Button } from '$lib/components/ui/button';
	import EmptyState from '$lib/layout/EmptyState.svelte';
	import { app } from '$lib/state/app.svelte';
	import FileChangeRow from './FileChangeRow.svelte';
	import { projectJournal, type ProjectJournal } from './journal.svelte';
	import { EDIT_SUM_LABEL, formatDateLong, formatDateTime, kindLabel, shortId } from './format';
	import type { PatchSource } from './types';

	const journal = $derived.by<ProjectJournal | null>(() => {
		const projectId = app.currentProjectId;
		return projectId ? projectJournal(projectId) : null;
	});

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

{#if !journal}
	<EmptyState title="Aucun projet ouvert" description="Choisissez un projet pour voir son historique de modifications." />
{:else if journal.error && journal.records.length === 0}
	<EmptyState title="Historique indisponible" description={journal.error}>
		{#snippet action()}
			<Button variant="outline" size="sm" onclick={() => journal?.load()}>Réessayer</Button>
		{/snippet}
	</EmptyState>
{:else if !journal.loaded}
	<p class="px-3 py-6 text-center text-sm text-muted-foreground">Chargement…</p>
{:else}
	<div class="flex flex-col gap-3">
		<header class="flex flex-col gap-1 text-xs text-muted-foreground">
			{#if journal.info?.since}
				<p>Journal tenu depuis le {formatDateLong(journal.info.since)}.</p>
			{:else}
				<p>Aucune modification enregistrée pour ce projet.</p>
			{/if}
			{#if journal.info?.coverage_note}
				<p class="text-pretty">{journal.info.coverage_note}</p>
			{/if}
			<p>Chiffres : <span class="italic">{EDIT_SUM_LABEL}</span>.</p>
		</header>

		{#if journal.records.length === 0}
			<EmptyState
				title="Aucune modification"
				description="Les modifications faites par les agents dans ce projet apparaîtront ici."
			/>
		{:else}
			{#each journal.records as record (record.id)}
				<article class="rounded-lg border border-border bg-card">
					<header class="flex flex-col gap-0.5 px-3 pt-2">
						<div class="flex items-baseline justify-between gap-2">
							<h3 class="min-w-0 truncate text-sm font-medium">{sessionTitle(record.session_id)}</h3>
							<time class="shrink-0 text-xs tabular-nums text-muted-foreground">{formatDateTime(record.at)}</time>
						</div>
						<p class="truncate text-xs text-muted-foreground">
							{record.tool}{record.agent_id ? ` · Agent ${shortId(record.agent_id)}` : ''}{record.changeset_id
								? ` · Changeset ${shortId(record.changeset_id)}`
								: ''}
						</p>
					</header>
					<div class="mt-1 border-t border-border px-3">
						{#each record.files as file (file.path)}
							<FileChangeRow
								path={file.path}
								status={kindLabel(file.kind)}
								added={file.added}
								removed={file.removed}
								binary={file.binary}
								countLabel={EDIT_SUM_LABEL}
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
		{/if}
	</div>
{/if}
