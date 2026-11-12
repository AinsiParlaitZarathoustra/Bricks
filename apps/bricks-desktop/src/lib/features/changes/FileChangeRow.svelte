<script lang="ts">
	import { t } from '$lib/i18n/i18n.svelte';
	import type { Snippet } from 'svelte';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import DiffStat from '$lib/layout/DiffStat.svelte';
	import FileIcon from '$lib/features/file-icons/FileIcon.svelte';
	import { errorMessage, type FileDiff } from '$lib/ipc/client';
	import DiffView from './DiffView.svelte';
	import { baseName, dirName } from './format';
	import type { PatchSource } from './types';

	interface Props {
		path: string;
		/** Name shown for the file. Default: the path, its folder dimmed. In a tree: the last part. */
		name?: string;
		/** Change label shown beside the path, e.g. « Modifié ». */
		status?: string;
		/** Added and removed counts. `null` or absent: not known, nothing is shown. */
		added?: number | null;
		removed?: number | null;
		binary?: boolean;
		/** Short labels of the agents that made the change. */
		agents?: string[];
		/** What the counts measure. Shown next to the figures. */
		countLabel?: string;
		/** Extra line under the path, e.g. the original path of a rename. */
		note?: string;
				/** Patches loaded when the row is expanded. None: the row cannot expand. */
		patches?: PatchSource[];
		/** Controls shown beside the row, outside its toggle (e.g. a link). */
		actions?: Snippet;
	}

	let {
		path,
		name,
		status,
		added,
		removed,
		binary = false,
		agents = [],
		countLabel,
				note,
		patches = [],
		actions
	}: Props = $props();

	let expanded = $state(false);
	let loaded = $state<Record<string, { diff?: FileDiff; error?: string }>>({});
	/** Keys already requested. Plain, not reactive: it only prevents duplicate loads. */
	const requested = new Set<string>();

	const expandable = $derived(patches.length > 0);
	const hasCounts = $derived(!binary && added != null && removed != null);

	// Load the patches of an expanded row, once per key. A changed key (for
	// example a newer git status) loads again on its own.
	$effect(() => {
		if (!expanded) return;
		for (const source of patches) {
			if (requested.has(source.key)) continue;
			requested.add(source.key);
			source
				.load()
				.then((diff) => {
					loaded[source.key] = { diff };
				})
				.catch((e: unknown) => {
					loaded[source.key] = { error: errorMessage(e) };
					requested.delete(source.key);
				});
		}
	});
</script>

<div class="min-w-0">
	<div class="flex min-w-0 items-center gap-1">
		<Button
			variant="ghost"
			size="sm"
			class="h-auto min-h-7 min-w-0 flex-1 justify-start py-1 text-ui font-normal disabled:opacity-100 aria-expanded:bg-element-selected"
			aria-expanded={expandable ? expanded : undefined}
			disabled={!expandable}
			title={path}
			onclick={() => (expanded = !expanded)}
		>
			<FileIcon {path} />
			<span class="flex min-w-0 flex-1 flex-col items-start gap-0.5">
				<span class="flex w-full min-w-0 items-center gap-2">
					<span class="min-w-0 truncate">
						{#if name}{name}{:else}{#if dirName(path)}<span class="text-muted-foreground">{dirName(path)}/</span>{/if}{baseName(path)}{/if}
					</span>
					<span class="ml-auto flex shrink-0 items-center gap-2 text-xs text-muted-foreground">
						{#if status}<span>{status}</span>{/if}
						{#if agents.length > 0}<span class="max-w-32 truncate">{agents.join(', ')}</span>{/if}
						{#if binary}
							<Badge variant="outline">{t('changes.binary')}</Badge>
						{:else if hasCounts}
							<DiffStat additions={added ?? 0} deletions={removed ?? 0} label={countLabel} />
						{/if}
					</span>
				</span>
				{#if note}<span class="max-w-full truncate text-xs text-muted-foreground">{note}</span>{/if}
			</span>
		</Button>
		{#if actions}
			<div class="shrink-0">
				{@render actions()}
			</div>
		{/if}
	</div>

	{#if expanded}
		<div class="flex flex-col gap-3 pt-1 pr-1 pb-3 pl-6">
			{#each patches as source (source.key)}
				{@const entry = loaded[source.key]}
				<div class="flex flex-col gap-1">
					<p class="text-xs text-muted-foreground">{source.title}</p>
					{#if entry?.diff}
						<DiffView patch={entry.diff.patch} truncated={entry.diff.truncated} binary={entry.diff.binary} />
					{:else if entry?.error}
						<p role="alert" class="text-xs text-destructive">{t('changes.couldNotLoadTheDiff', { error: entry.error })}</p>
					{:else}
						<p aria-live="polite" class="text-xs text-muted-foreground">{t('common.loading')}</p>
					{/if}
				</div>
			{/each}
		</div>
	{/if}
</div>
