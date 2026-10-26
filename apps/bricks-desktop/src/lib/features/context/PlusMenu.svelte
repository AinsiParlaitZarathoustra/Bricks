<script lang="ts">
	// « Plus »: the ADK inspectors of the shown conversation, from the top bar.
	import { app } from '$lib/state/app.svelte';
	import { ui } from '$lib/state/ui.svelte';
	import { buttonVariants } from '$lib/components/ui/button';
	import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
	import EllipsisIcon from '@lucide/svelte/icons/ellipsis';
	import { ADK_ACCESS_GROUPS } from './adkAccess';

	// The menu opens inspectors that read the shown conversation: none without one.
	const enabled = $derived(app.current !== null);
</script>

<DropdownMenu.Root>
	<DropdownMenu.Trigger
		class={[buttonVariants({ variant: 'ghost', size: 'sm' }), 'h-7 px-2 text-ui']}
		disabled={!enabled}
	>
		<EllipsisIcon aria-hidden="true" />
		Plus
	</DropdownMenu.Trigger>
	<DropdownMenu.Content align="end" class="min-w-44">
		{#each ADK_ACCESS_GROUPS as group, i (group.label)}
			{#if i > 0}
				<DropdownMenu.Separator />
			{/if}
			<DropdownMenu.Group>
				<DropdownMenu.Label>{group.label}</DropdownMenu.Label>
				{#each group.items as item (item.view)}
					<DropdownMenu.Item onSelect={() => ui.open(item.view)}>
						{item.label}
					</DropdownMenu.Item>
				{/each}
			</DropdownMenu.Group>
		{/each}
	</DropdownMenu.Content>
</DropdownMenu.Root>
