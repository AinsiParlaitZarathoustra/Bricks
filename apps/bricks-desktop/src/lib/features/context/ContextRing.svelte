<script lang="ts">
	import type { ContextStatus } from '$lib/ipc/engine';
	import { cn } from '$lib/utils.js';
	import { contextView } from './contextView';

	interface Props {
		/** Last context measure of the conversation, `null` when none. */
		status: ContextStatus | null;
		/** The measure no longer applies (model change, compaction, clear). */
		stale: boolean;
		/** Diameter in px. */
		size?: number;
		class?: string;
	}

	let { status, stale, size = 40, class: className }: Props = $props();

	const view = $derived(contextView(status, stale));

	const centre = $derived(
		view.kind === 'overflow'
			? '100'
			: view.percent !== null
				? String(Math.round(view.percent))
				: '—'
	);

	const tone = $derived(
		view.kind === 'overflow'
			? 'stroke-warning'
			: view.percent !== null && view.percent >= 90
				? 'stroke-warning'
				: 'stroke-primary'
	);
</script>

<span
	class={cn('relative inline-flex shrink-0 items-center justify-center', className)}
	style:width="{size}px"
	style:height="{size}px"
>
	<svg viewBox="0 0 36 36" class="size-full -rotate-90" role="img" aria-label={view.ariaLabel}>
		<circle cx="18" cy="18" r="15.5" fill="none" stroke-width="3" class="stroke-border" />
		{#if view.fraction > 0}
			<circle
				cx="18"
				cy="18"
				r="15.5"
				fill="none"
				stroke-width="3"
				stroke-linecap="round"
				pathLength="100"
				stroke-dasharray="{view.fraction * 100} 100"
				class={tone}
			/>
		{/if}
	</svg>
	<span
		aria-hidden="true"
		class={cn(
			'absolute font-mono font-medium tabular-nums',
			view.kind === 'overflow' ? 'text-warning' : 'text-foreground',
			view.percent === null && view.kind !== 'overflow' && 'text-muted-foreground'
		)}
		style:font-size="{Math.round(size * 0.26)}px"
	>
		{centre}
	</span>
</span>
