<script lang="ts">
	import type { Snippet } from 'svelte';
	import { cn } from '$lib/utils.js';

	interface Props {
		/** Visible heading; also names the group for assistive technology. */
		title: string;
		description?: string;
		class?: string;
		children: Snippet;
	}

	let { title, description, class: className, children }: Props = $props();

	const headingId = $props.id();
</script>

<section aria-labelledby={headingId} class={cn('flex flex-col gap-3', className)}>
	<div class="flex flex-col gap-1">
		<h3 id={headingId} class="text-[13px] font-semibold text-foreground">{title}</h3>
		{#if description}
			<p class="text-xs text-pretty text-muted-foreground">{description}</p>
		{/if}
	</div>
	<div class="flex flex-col divide-y divide-border rounded-lg border border-border bg-card text-card-foreground">
		{@render children()}
	</div>
</section>
