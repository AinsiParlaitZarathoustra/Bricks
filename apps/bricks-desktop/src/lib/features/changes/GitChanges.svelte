<script lang="ts">
	import { t } from '$lib/i18n/i18n.svelte';
	import { untrack } from 'svelte';
	import RefreshCw from '@lucide/svelte/icons/refresh-cw';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import EmptyState from '$lib/layout/EmptyState.svelte';
	import SectionCard from '$lib/layout/SectionCard.svelte';
	import { errorMessage, gitDiff, gitShow, type FileDiff, type GitArea, type GitCommit } from '$lib/ipc/client';
	import { app } from '$lib/state/app.svelte';
	import FileChangeRow from './FileChangeRow.svelte';
	import { gitState, type GitState } from './git.svelte';
	import { countPatch } from './patch';
	import { formatDateTime, gitCodeLabel, shortId } from './format';
	import type { PatchSource } from './types';

	const AREAS: { area: GitArea; title: string }[] = [
		{ area: 'staged', get title() { return t('changes.staged') } },
		{ area: 'unstaged', get title() { return t('changes.unstaged') } },
		{ area: 'untracked', get title() { return t('changes.untracked') } },
		{ area: 'conflicted', get title() { return t('changes.conflicts') } },
	];

	type CommitDetail =
		| { status: 'loading' }
		| { status: 'error'; error: string }
		| { status: 'ready'; files: { diff: FileDiff; added: number; removed: number }[] };

	let expanded = $state<Record<string, boolean>>({});
	let details = $state<Record<string, CommitDetail>>({});

	const git = $derived.by<GitState | null>(() => {
		const projectId = app.currentProjectId;
		return projectId ? gitState(projectId) : null;
	});

	// Refresh when the panel shows a project (and on each project switch).
	$effect(() => {
		const state = git;
		if (!state) return;
		untrack(() => {
			void state.refresh();
			if (!state.commitsLoaded && !state.commitsLoading) void state.loadCommits();
		});
	});

	// Refresh when a run of this project ends: its edits are on disk now.
	const runningHere = $derived(
		[...app.conversations.values()].filter((c) => c.projectId === app.currentProjectId && c.running).length
	);
	let previousRunning = 0;
	$effect(() => {
		const now = runningHere;
		if (previousRunning > now) untrack(() => void git?.refresh());
		previousRunning = now;
	});

	const sections = $derived(
		AREAS.map((s) => ({
			...s,
			entries: git?.status?.entries.filter((e) => e.area === s.area) ?? [],
		})).filter((s) => s.entries.length > 0),
	);

	const ahead = $derived(git?.status?.ahead ?? 0);
	const behind = $derived(git?.status?.behind ?? 0);

	async function toggleCommit(commit: GitCommit) {
		const open = !expanded[commit.hash];
		expanded[commit.hash] = open;
		if (!open || details[commit.hash]?.status === 'ready') return;
		const projectId = app.currentProjectId;
		if (!projectId) return;
		details[commit.hash] = { status: 'loading' };
		try {
			const files = await gitShow(projectId, commit.hash);
			details[commit.hash] = {
				status: 'ready',
				files: files.map((diff) => ({ diff, ...countPatch(diff.patch) })),
			};
		} catch (e) {
			details[commit.hash] = { status: 'error', error: errorMessage(e) };
		}
	}

	function diffSource(path: string, area: GitArea): PatchSource[] {
		const projectId = app.currentProjectId;
		if (!projectId) return [];
		// The revision changes with each status refresh, so a stale diff reloads.
		return [
			{
				key: `diff-${git?.revision ?? 0}`,
				title: t('common.diff'),
				load: () => gitDiff(projectId, path, area),
			},
		];
	}
</script>

<svelte:window onfocus={() => void git?.refresh()} />

{#if !app.currentProjectId}
	<EmptyState title={t('common.noOpenProject')} description={t('changes.chooseAProjectToSeeIts')} />
{:else if git?.statusError && !git.status}
	<EmptyState title={t('changes.gitUnavailable')} description={git.statusError}>
		{#snippet action()}
			<Button variant="outline" size="sm" onclick={() => git?.refresh()}>{t('common.retry')}</Button>
		{/snippet}
	</EmptyState>
{:else if git?.status && !git.status.is_repo}
	<EmptyState title={t('common.gitIsntInstalledInThisFolder')} />
{:else}
	<div class="flex flex-col gap-4">
		<header class="flex items-center gap-2">
			<div class="flex min-w-0 flex-1 flex-wrap items-center gap-x-2 gap-y-1">
				{#if git?.status?.detached}
					<Badge variant="outline">{t('changes.detachedHead', { value: git.status.head ? ` · ${git.status.head}` : '' })}</Badge>
				{:else if git?.status?.branch}
					<span class="truncate font-mono text-sm">{git.status.branch}</span>
				{:else if git?.status}
					<span class="text-sm text-muted-foreground">{t('changes.noBranchYet')}</span>
				{/if}
				{#if ahead > 0}<Badge variant="secondary">{t('changes.toPush', { ahead })}</Badge>{/if}
				{#if behind > 0}<Badge variant="secondary">{t('changes.toPull', { behind })}</Badge>{/if}
			</div>
			<Button
				variant="ghost"
				size="icon-sm"
				aria-label={t('common.refresh')}
				title={t('common.refresh')}
				disabled={git?.statusLoading}
				onclick={() => git?.refresh()}
			>
				<RefreshCw class={git?.statusLoading ? 'size-4 animate-spin' : 'size-4'} aria-hidden="true" />
			</Button>
		</header>

		{#if !git?.status}
			<p class="px-3 py-6 text-center text-sm text-muted-foreground">{t('common.loading')}</p>
		{:else if sections.length === 0}
			<p class="text-sm text-muted-foreground">{t('changes.noChangesTheWorkingTreeIs')}</p>
		{:else}
			{#each sections as section (section.area)}
				<SectionCard title={section.title} count={section.entries.length}>
					{#each section.entries as entry (`${entry.area}:${entry.path}`)}
						<FileChangeRow
							path={entry.path}
							status={gitCodeLabel(entry.code)}
							added={entry.added}
							removed={entry.removed}
							binary={entry.binary}
							countLabel={t('common.lines')}
							note={entry.orig_path ? t('changes.from', { origPath: entry.orig_path }) : undefined}
							patches={diffSource(entry.path, entry.area)}
						/>
					{/each}
				</SectionCard>
			{/each}
		{/if}

		<SectionCard title={t('changes.commits')} count={git?.commitsLoaded ? git.commits.length : undefined}>
			{#if git?.commitsError}
				<p role="alert" class="text-xs text-destructive">{t('changes.historyUnavailable', { commitsError: git.commitsError })}</p>
			{:else if !git?.commitsLoaded}
				<p class="py-2 text-sm text-muted-foreground">{t('common.loading')}</p>
			{:else if git.commits.length === 0}
				<p class="py-2 text-sm text-muted-foreground">{t('changes.noCommitsYet')}</p>
			{:else}
				<ul class="flex flex-col">
					{#each git.commits as commit (commit.hash)}
						<li class="border-b border-border py-1.5 last:border-b-0">
							<button
								type="button"
								class="flex w-full min-w-0 items-baseline gap-2 rounded-sm text-left outline-none focus-visible:ring-2 focus-visible:ring-ring"
								aria-expanded={expanded[commit.hash] ?? false}
								onclick={() => toggleCommit(commit)}
							>
								<span class="shrink-0 font-mono text-xs text-muted-foreground">{commit.short}</span>
								<span class="min-w-0 flex-1 truncate text-sm">{commit.subject || t('changes.noMessage')}</span>
							</button>
							<p class="pl-[3.25rem] text-xs text-muted-foreground">
								{commit.author} · <time class="tabular-nums">{formatDateTime(commit.at)}</time>
							</p>
							{#if expanded[commit.hash]}
								{@const detail = details[commit.hash]}
								<div class="mt-1 pl-2">
									{#if detail?.status === 'loading' || !detail}
										<p class="text-xs text-muted-foreground">{t('common.loading')}</p>
									{:else if detail.status === 'error'}
										<p role="alert" class="text-xs text-destructive">{t('changes.couldNotLoadTheCommit', { error: detail.error })}</p>
									{:else if detail.files.length === 0}
										<p class="text-xs text-muted-foreground">{t('changes.noFileOfThisProjectIn')}</p>
									{:else}
										{#each detail.files as file (file.diff.path)}
											<FileChangeRow
												path={file.diff.path}
												added={file.added}
												removed={file.removed}
												binary={file.diff.binary}
												countLabel={t('common.lines')}
												patches={[
													{ key: 'diff', title: t('changes.diff', { hash: shortId(commit.hash) }), load: async () => file.diff },
												]}
											/>
										{/each}
									{/if}
								</div>
							{/if}
						</li>
					{/each}
				</ul>
				{#if !git.commitsExhausted}
					<div class="flex justify-center pt-2">
						<Button
							variant="outline"
							size="sm"
							disabled={git.commitsLoading}
							onclick={() => git?.loadCommits(true)}
						>
							{git.commitsLoading ? t('common.loading') : t('changes.loadMore')}
						</Button>
					</div>
				{/if}
			{/if}
		</SectionCard>
	</div>
{/if}
