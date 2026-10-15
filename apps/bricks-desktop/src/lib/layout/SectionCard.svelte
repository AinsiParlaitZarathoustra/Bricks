<script lang="ts">
	import type { Snippet } from 'svelte';
	import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';
	import * as Collapsible from '$lib/components/ui/collapsible/index.js';
	import { cn } from '$lib/utils.js';
	import type { HeadingLevel } from './types.js';

	interface Props {
		title: string;
		description?: string;
		/** Optional figure shown on the right of the title, e.g. a number of items. */
		count?: number;
		/** Whether the body is shown. Bindable, defaults to open. */
		open?: boolean;
		/** Heading level of the title (default 3: a section inside a panel whose title is h2). */
		headingLevel?: HeadingLevel;
		/** Controls shown at the right of the header, next to the toggle. Click them without collapsing the card. */
		actions?: Snippet;
		children: Snippet;
		class?: string;
	}

	let {
		title,
		description,
		count,
		open = $bindable(true),
		headingLevel = 3,
		actions,
		children,
		class: className
	}: Props = $props();

	const headingTag = $derived(`h${headingLevel}` as const);
</script>

<Collapsible.Root
	bind:open
	class={cn('rounded-lg border border-border bg-card text-card-foreground', className)}
>
	<div class="flex items-center gap-1 pr-1">
		<svelte:element this={headingTag} class="min-w-0 flex-1 text-sm font-medium">
			<Collapsible.Trigger
				class="group flex w-full min-w-0 items-center gap-2 rounded-lg px-3 py-2 text-left outline-none focus-visible:ring-2 focus-visible:ring-ring"
			>
				<ChevronDownIcon
					class="size-4 shrink-0 text-muted-foreground transition-transform duration-150 group-data-[state=open]:rotate-180"
					aria-hidden="true"
				/>
				<span class="min-w-0 flex-1 truncate">{title}</span>
				{#if count !== undefined}
					<span class="font-mono text-xs font-normal tabular-nums text-muted-foreground">{count}</span>
				{/if}
			</Collapsible.Trigger>
		</svelte:element>
		{#if actions}
			<div class="flex shrink-0 items-center">
				{@render actions()}
			</div>
		{/if}
	</div>
	<Collapsible.Content>
		{#if description}
			<p class="px-3 pb-1 text-xs text-pretty text-muted-foreground">{description}</p>
		{/if}
		<div class="border-t border-border px-3 py-2">
			{@render children()}
		</div>
	</Collapsible.Content>
</Collapsible.Root>
