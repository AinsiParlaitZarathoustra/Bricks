<script lang="ts">
	import { cn } from '$lib/utils.js';
	import type { HTMLAttributes } from 'svelte/elements';

	interface Props extends HTMLAttributes<HTMLSpanElement> {
		/** Number of added units (lines, files...). Negative or fractional values are clamped to whole numbers >= 0. */
		additions: number;
		/** Number of removed units. */
		deletions: number;
		/** What the numbers count, e.g. "lignes" or "fichiers". Shown after the figures. */
		label?: string;
	}

	let { additions, deletions, label, class: className, ...restProps }: Props = $props();

	const formatter = new Intl.NumberFormat('fr-FR');
	const add = $derived(Math.max(0, Math.trunc(additions) || 0));
	const del = $derived(Math.max(0, Math.trunc(deletions) || 0));
	const addText = $derived(formatter.format(add));
	const delText = $derived(formatter.format(del));
	const spoken = $derived(
		`${addText} ajout${add > 1 ? 's' : ''}, ${delText} suppression${del > 1 ? 's' : ''}${label ? `, ${label}` : ''}`
	);
</script>

<span class={cn('inline-flex items-baseline gap-1.5 whitespace-nowrap text-xs tabular-nums', className)} {...restProps}>
	<span aria-hidden="true" class="inline-flex items-baseline gap-1 font-mono">
		<span class={add > 0 ? 'text-diff-add' : 'text-muted-foreground'}>+{addText}</span>
		<span class={del > 0 ? 'text-diff-del' : 'text-muted-foreground'}>−{delText}</span>
	</span>
	{#if label}
		<span aria-hidden="true" class="text-muted-foreground">{label}</span>
	{/if}
	<span class="sr-only">{spoken}</span>
</span>
