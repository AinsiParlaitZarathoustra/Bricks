<script lang="ts" module>
	export type StatusState = 'idle' | 'running' | 'waiting' | 'success' | 'error' | 'cancelled';

	export const STATUS_LABELS: Record<StatusState, string> = {
		idle: 'Inactif',
		running: 'En cours',
		waiting: 'En attente',
		success: 'Terminé',
		error: 'Erreur',
		cancelled: 'Annulé'
	};
</script>

<script lang="ts">
	import { cn } from '$lib/utils.js';
	import type { HTMLAttributes } from 'svelte/elements';

	interface Props extends HTMLAttributes<HTMLSpanElement> {
		status: StatusState;
		/** Overrides the default French label. */
		label?: string;
		/** Shows the label as visible text next to the dot. Otherwise it is screen-reader only. */
		showLabel?: boolean;
	}

	let { status, label, showLabel = false, class: className, ...restProps }: Props = $props();

	const text = $derived(label ?? STATUS_LABELS[status]);

	const DOT: Record<StatusState, string> = {
		idle: 'bg-status-idle',
		running: 'bg-info',
		waiting: 'bg-warning',
		success: 'bg-success',
		error: 'bg-destructive',
		// Cancelled is a hollow ring so it is not confused with idle by colour alone.
		cancelled: 'border border-muted-foreground bg-transparent'
	};
</script>

<span class={cn('inline-flex items-center gap-1.5 text-xs text-muted-foreground', className)} {...restProps}>
	<span
		aria-hidden="true"
		data-status={status}
		class={cn('inline-block size-2 shrink-0 rounded-full', DOT[status], status === 'running' && 'animate-pulse')}
	></span>
	{#if showLabel}
		<span>{text}</span>
	{:else}
		<span class="sr-only">{text}</span>
	{/if}
</span>
