<script lang="ts">
	import type { Snippet } from 'svelte';
	import { cn } from '$lib/utils.js';

	interface Props {
		/** Visible heading; also names the group for assistive technology. */
		title: string;
		description?: string;
		class?: string;
		/** A small control at the right of the title (an icon button). */
		action?: Snippet;
		children: Snippet;
	}

	let { title, description, class: className, action, children }: Props = $props();

	const headingId = $props.id();
</script>

<section aria-labelledby={headingId} class={cn('flex flex-col gap-3', className)}>
	<div class="flex flex-col gap-1">
		<div class="flex min-h-6 items-center justify-between gap-2">
			<h3 id={headingId} class="text-ui font-semibold text-foreground">{title}</h3>
			{@render action?.()}
		</div>
		{#if description}
			<p class="text-xs text-pretty text-muted-foreground">{description}</p>
		{/if}
	</div>
	<div class="flex flex-col divide-y divide-border rounded-lg border border-border bg-transparent text-foreground">
		{@render children()}
	</div>
</section>
