<script lang="ts">
	import { onDestroy } from 'svelte';
	import { toast } from 'svelte-sonner';
	import { app } from '$lib/state/app.svelte';
	import { errorMessage, gitStatus } from '$lib/ipc/client';
	import type { GitStatus, Project } from '$lib/ipc/client';
	import type { Conversation } from '$lib/state/conversation.svelte';
	import { Button } from '$lib/components/ui/button';
	import SectionCard from '$lib/layout/SectionCard.svelte';
	import CheckIcon from '@lucide/svelte/icons/check';
	import CopyIcon from '@lucide/svelte/icons/copy';
	import FolderIcon from '@lucide/svelte/icons/folder';
	import FolderKanbanIcon from '@lucide/svelte/icons/folder-kanban';
	import GitBranchIcon from '@lucide/svelte/icons/git-branch';
	import TriangleAlertIcon from '@lucide/svelte/icons/triangle-alert';

	interface Props {
		project: Project;
		/** The shown conversation, or `null` when none is open. */
		conversation: Conversation | null;
	}

	let { project, conversation }: Props = $props();

	const projectId = $derived(project.id);
	const projectMissing = $derived(project.missing);
	const projectName = $derived(project.name);
	const projectPath = $derived(project.path);

	interface GitCounts {
		staged: number;
		unstaged: number;
		untracked: number;
		conflicted: number;
	}

	type GitResult =
		| { kind: 'error'; message: string }
		| { kind: 'ready'; status: GitStatus; counts: GitCounts };

	/** Last git result, tagged with the project it describes. */
	let git = $state<{ projectId: string; result: GitResult } | null>(null);
	/** `null` while the first read of the current project is pending. */
	const gitView = $derived(git && git.projectId === projectId ? git.result : null);
	/** Only the latest request may update the view (project switches, focus). */
	let requestSeq = 0;

	function countEntries(status: GitStatus): GitCounts {
		const counts: GitCounts = { staged: 0, unstaged: 0, untracked: 0, conflicted: 0 };
		for (const entry of status.entries) counts[entry.area] += 1;
		return counts;
	}

	function dirtyCount(c: GitCounts): number {
		return c.staged + c.unstaged + c.untracked + c.conflicted;
	}

	function headLabel(s: GitStatus): string {
		if (s.detached) return s.head ? `HEAD détachée (${s.head})` : 'HEAD détachée';
		const branch = s.branch ?? 'Branche inconnue';
		return s.head ? branch : `${branch} (aucun commit)`;
	}

	async function refreshGit(id: string) {
		const seq = ++requestSeq;
		try {
			const status = await gitStatus(id);
			if (seq === requestSeq)
				git = { projectId: id, result: { kind: 'ready', status, counts: countEntries(status) } };
		} catch (e) {
			if (seq === requestSeq)
				git = { projectId: id, result: { kind: 'error', message: errorMessage(e) } };
		}
	}

	// Project change: read the workspace state of the new folder.
	$effect(() => {
		const id = projectId;
		if (!projectMissing) void refreshGit(id);
	});

	// A run finished in a conversation of this project: the working tree may
	// have changed. Plain memory of each conversation's last state (not UI state).
	const lastRunning: Record<string, boolean> = {};
	$effect(() => {
		const id = projectId;
		let finished = false;
		for (const c of app.conversations.values()) {
			const running = c.running;
			if (c.projectId === id && !running && lastRunning[c.sessionId] === true) finished = true;
			lastRunning[c.sessionId] = running;
		}
		if (finished && !projectMissing) void refreshGit(id);
	});

	let copied = $state(false);
	let copyTimer: ReturnType<typeof setTimeout> | undefined;

	async function copyPath() {
		try {
			await navigator.clipboard.writeText(projectPath);
			copied = true;
			clearTimeout(copyTimer);
			copyTimer = setTimeout(() => (copied = false), 1500);
		} catch (e) {
			toast.error('Copie impossible', { description: errorMessage(e) });
		}
	}

	onDestroy(() => clearTimeout(copyTimer));

	const pendingText = $derived.by(() => {
		const change = conversation?.pendingChange;
		if (!change) return null;
		const what = change.what === 'model' ? 'Modèle' : 'Raisonnement';
		const when = change.applies === 'next_turn' ? 'au prochain tour' : 'à la prochaine exécution';
		return `${what} : changement appliqué ${when}.`;
	});
</script>

<svelte:window
	onfocus={() => {
		if (!projectMissing) void refreshGit(projectId);
	}}
/>

<SectionCard title="Environnement">
	<dl class="grid grid-cols-[1rem_minmax(0,1fr)] items-start gap-x-2.5 gap-y-2 text-xs">
		<dt class="flex h-4 items-center text-muted-foreground">
			<FolderKanbanIcon class="size-3.5" aria-hidden="true" />
			<span class="sr-only">Projet</span>
		</dt>
		<dd class="min-w-0 truncate font-medium" title={projectName}>{projectName}</dd>

		<dt class="flex h-4 items-center text-muted-foreground">
			<FolderIcon class="size-3.5" aria-hidden="true" />
			<span class="sr-only">Dossier</span>
		</dt>
		<dd class="flex min-w-0 items-center gap-1">
			<span class="min-w-0 flex-1 truncate font-mono text-xs" title={projectPath}>{projectPath}</span>
			<Button variant="ghost" size="icon-xs" aria-label="Copier le chemin du dossier" onclick={copyPath}>
				{#if copied}
					<CheckIcon aria-hidden="true" />
				{:else}
					<CopyIcon aria-hidden="true" />
				{/if}
			</Button>
		</dd>

		<dt class="flex h-4 items-center text-muted-foreground">
			<GitBranchIcon class="size-3.5" aria-hidden="true" />
			<span class="sr-only">Git</span>
		</dt>
		<dd class="min-w-0">
			{#if projectMissing}
				<span class="text-muted-foreground">Dossier introuvable.</span>
			{:else if gitView === null}
				<span class="text-muted-foreground">Lecture de l’état du dépôt…</span>
			{:else if gitView.kind === 'error'}
				<span class="flex items-start gap-1.5 text-warning">
					<TriangleAlertIcon class="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
					<span class="min-w-0 text-pretty">Git indisponible : {gitView.message}</span>
				</span>
			{:else if !gitView.status.is_repo}
				<span class="text-muted-foreground">Ce dossier n’est pas un dépôt Git.</span>
			{:else}
				<div class="flex min-w-0 flex-col gap-1">
					<span class="truncate font-mono">{headLabel(gitView.status)}</span>

					{#if gitView.status.ahead > 0 || gitView.status.behind > 0}
						<span class="flex flex-wrap gap-x-3 tabular-nums text-muted-foreground">
							{#if gitView.status.ahead > 0}<span>↑ {gitView.status.ahead} à publier</span>{/if}
							{#if gitView.status.behind > 0}<span>↓ {gitView.status.behind} à récupérer</span>{/if}
						</span>
					{/if}

					{#if dirtyCount(gitView.counts) === 0}
						<span class="text-success">Espace de travail propre</span>
					{:else}
						<ul class="flex flex-wrap gap-x-3 gap-y-0.5 tabular-nums text-muted-foreground">
							{#if gitView.counts.staged > 0}<li>Indexés : {gitView.counts.staged}</li>{/if}
							{#if gitView.counts.unstaged > 0}<li>Non indexés : {gitView.counts.unstaged}</li>{/if}
							{#if gitView.counts.untracked > 0}<li>Non suivis : {gitView.counts.untracked}</li>{/if}
							{#if gitView.counts.conflicted > 0}
								<li class="text-destructive">En conflit : {gitView.counts.conflicted}</li>
							{/if}
						</ul>
					{/if}
				</div>
			{/if}
		</dd>
	</dl>

	{#if pendingText}
		<p class="mt-2 text-xs text-pretty text-muted-foreground">{pendingText}</p>
	{/if}
</SectionCard>
