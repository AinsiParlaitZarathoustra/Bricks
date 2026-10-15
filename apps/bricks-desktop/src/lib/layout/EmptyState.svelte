<script lang="ts">
	import type { Snippet } from 'svelte';
	import { cn } from '$lib/utils.js';
	import type { HeadingLevel, IconComponent } from './types.js';

	interface Props {
		title: string;
		description?: string;
		/** A lucide icon component. Decorative: the title carries the meaning. */
		icon?: IconComponent;
		/** Heading level of the title (default 2). Use the level that fits the region's outline. */
		headingLevel?: HeadingLevel;
		/** Optional call to action, e.g. a Button. Rendered only when provided. */
		action?: Snippet;
		class?: string;
	}

	let { title, description, icon: Icon, headingLevel = 2, action, class: className }: Props = $props();

	const headingTag = $derived(`h${headingLevel}` as const);
</script>

<div class={cn('flex flex-col items-center justify-center gap-3 px-6 py-10 text-center', className)}>
	{#if Icon}
		<span class="flex size-10 items-center justify-center rounded-full bg-muted text-muted-foreground">
			<Icon class="size-5" aria-hidden="true" />
		</span>
	{/if}
	<div class="flex max-w-xs flex-col gap-1">
		<svelte:element this={headingTag} class="text-sm font-medium text-foreground">{title}</svelte:element>
		{#if description}
			<p class="text-sm text-pretty text-muted-foreground">{description}</p>
		{/if}
	</div>
	{#if action}
		<div class="mt-1">
			{@render action()}
		</div>
	{/if}
</div>
