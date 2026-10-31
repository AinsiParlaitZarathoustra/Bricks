<script lang="ts">
	// « Plus »: the ADK inspectors of the shown conversation, from the top bar.
	import { app } from '$lib/state/app.svelte';
	import { ui } from '$lib/state/ui.svelte';
	import { buttonVariants } from '$lib/components/ui/button';
	import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
	import { DropdownMenu as DropdownMenuPrimitive } from 'bits-ui';
	import EllipsisIcon from '@lucide/svelte/icons/ellipsis';
	import { ADK_ACCESS_GROUPS } from './adkAccess';

	// The menu opens inspectors that read the shown conversation: none without one.
	const enabled = $derived(app.current !== null);
	const keepAwake = $derived(app.current?.keepAwake ?? false);

	function setKeepAwake(on: boolean) {
		if (app.current) app.current.keepAwake = on;
	}
</script>

<DropdownMenu.Root>
	<DropdownMenu.Trigger
		class={[buttonVariants({ variant: 'ghost', size: 'sm' }), 'h-7 px-2 text-ui']}
		disabled={!enabled}
	>
		<EllipsisIcon aria-hidden="true" />
		Plus
	</DropdownMenu.Trigger>
	<DropdownMenu.Content align="end" class="min-w-64">
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
		<DropdownMenu.Separator />
		<DropdownMenuPrimitive.CheckboxItem
			checked={keepAwake}
			onCheckedChange={setKeepAwake}
			closeOnSelect={false}
			class="flex cursor-default items-center gap-3 rounded-sm px-2 py-1.5 outline-hidden select-none focus:bg-accent focus:text-accent-foreground"
		>
			<span class="flex min-w-0 flex-1 flex-col">
				<span class="text-sm">Maintenir l’ordinateur actif</span>
				<span class="text-xs text-muted-foreground">Uniquement pour cette session</span>
			</span>
			<span
				aria-hidden="true"
				class={[
					'relative inline-flex h-[18px] w-8 shrink-0 items-center rounded-full transition-colors',
					keepAwake ? 'bg-primary' : 'bg-input dark:bg-input/80'
				]}
			>
				<span
					class={[
						'block size-4 rounded-full transition-transform',
						keepAwake ? 'translate-x-[calc(100%-2px)] bg-background dark:bg-primary-foreground' : 'translate-x-px bg-background dark:bg-foreground'
					]}
				></span>
			</span>
		</DropdownMenuPrimitive.CheckboxItem>
	</DropdownMenu.Content>
</DropdownMenu.Root>
