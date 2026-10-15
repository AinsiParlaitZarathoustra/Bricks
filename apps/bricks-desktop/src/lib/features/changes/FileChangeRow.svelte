<script lang="ts">
	import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';
	import { Badge } from '$lib/components/ui/badge';
	import DiffStat from '$lib/layout/DiffStat.svelte';
	import { errorMessage, type FileDiff } from '$lib/ipc/client';
	import { cn } from '$lib/utils.js';
	import DiffView from './DiffView.svelte';
	import { baseName, dirName } from './format';
	import type { PatchSource } from './types';

	interface Props {
		path: string;
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
	}

	let {
		path,
		status,
		added,
		removed,
		binary = false,
		agents = [],
		countLabel,
		note,
		patches = []
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

<div class="border-b border-border last:border-b-0">
	<button
		type="button"
		class="flex w-full min-w-0 items-start gap-2 rounded-sm px-1 py-2 text-left outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-default"
		aria-expanded={expandable ? expanded : undefined}
		disabled={!expandable}
		onclick={() => (expanded = !expanded)}
	>
		{#if expandable}
			<ChevronDownIcon
				class={cn('mt-0.5 size-4 shrink-0 text-muted-foreground transition-transform duration-150', expanded && 'rotate-180')}
				aria-hidden="true"
			/>
		{:else}
			<span class="size-4 shrink-0" aria-hidden="true"></span>
		{/if}
		<span class="flex min-w-0 flex-1 flex-col gap-0.5">
			<span class="truncate font-mono text-xs" title={path}>
				{#if dirName(path)}<span class="text-muted-foreground">{dirName(path)}/</span>{/if}{baseName(path)}
			</span>
			<span class="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted-foreground">
				{#if status}<span>{status}</span>{/if}
				{#if agents.length > 0}<span>{agents.join(', ')}</span>{/if}
				{#if binary}
					<Badge variant="outline">Binaire</Badge>
				{:else if hasCounts}
					<DiffStat additions={added ?? 0} deletions={removed ?? 0} label={countLabel} />
				{/if}
			</span>
			{#if note}<span class="truncate text-xs text-muted-foreground">{note}</span>{/if}
		</span>
	</button>

	{#if expanded}
		<div class="flex flex-col gap-3 pr-1 pb-3 pl-6">
			{#each patches as source (source.key)}
				{@const entry = loaded[source.key]}
				<div class="flex flex-col gap-1">
					<p class="text-xs text-muted-foreground">{source.title}</p>
					{#if entry?.diff}
						<DiffView patch={entry.diff.patch} truncated={entry.diff.truncated} binary={entry.diff.binary} />
					{:else if entry?.error}
						<p role="alert" class="text-xs text-destructive">Impossible de charger le diff : {entry.error}</p>
					{:else}
						<p aria-live="polite" class="text-xs text-muted-foreground">Chargement…</p>
					{/if}
				</div>
			{/each}
		</div>
	{/if}
</div>
