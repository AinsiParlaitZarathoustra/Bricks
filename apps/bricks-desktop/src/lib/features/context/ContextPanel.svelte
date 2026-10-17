<script lang="ts">
	// Right panel: project environment, context occupation, pending approvals,
	// the sub-agents of the shown conversation, and the « Plus » menu that opens
	// the existing ADK inspectors. Owner: Haiku 6 (Sprint 14), menu: Haiku F4 (Sprint 14.3).
	import { app } from '$lib/state/app.svelte';
	import { ui } from '$lib/state/ui.svelte';
	import { ScrollArea } from '$lib/components/ui/scroll-area';
	import { buttonVariants } from '$lib/components/ui/button';
	import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
	import EmptyState from '$lib/layout/EmptyState.svelte';
	import SectionCard from '$lib/layout/SectionCard.svelte';
	import AgentList from '$lib/features/agents/AgentList.svelte';
	import { ADK_ACCESS_GROUPS } from './adkAccess';
	import ContextCard from './ContextCard.svelte';
	import PendingApprovals from './PendingApprovals.svelte';
	import ProjectEnvironment from './ProjectEnvironment.svelte';
	import EllipsisIcon from '@lucide/svelte/icons/ellipsis';
	import FolderOpenIcon from '@lucide/svelte/icons/folder-open';

	const project = $derived(app.currentProject);
	const conversation = $derived(app.current);
	const approvals = $derived(conversation?.pendingApprovals ?? []);
	const agentCount = $derived(conversation?.agents.size ?? 0);
	// The menu opens inspectors that read the shown conversation: none without one.
	const menuEnabled = $derived(conversation !== null);

	// The scroll viewport takes keyboard focus and has a name, so keyboard users
	// can scroll the panel even when no control in view is focusable.
	let viewport = $state<HTMLElement | null>(null);
	$effect(() => {
		if (!viewport) return;
		viewport.tabIndex = 0;
		viewport.setAttribute('role', 'region');
		viewport.setAttribute('aria-label', 'Contexte du projet');
	});
</script>

<div class="h-full">
	{#if !project}
		<EmptyState title="Aucun projet ouvert" icon={FolderOpenIcon} />
	{:else}
		<ScrollArea class="h-full" bind:viewportRef={viewport}>
			<div class="flex flex-col gap-3 p-3">
				<div class="flex justify-end">
					<DropdownMenu.Root>
						<DropdownMenu.Trigger
							class={buttonVariants({ variant: 'ghost', size: 'sm' })}
							disabled={!menuEnabled}
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
				</div>

				{#if conversation}
					<PendingApprovals {approvals} />
				{/if}

				<ProjectEnvironment {project} {conversation} />

				{#if conversation}
					<ContextCard {conversation} />

					<SectionCard title="Sous-agents" count={agentCount}>
						<AgentList {conversation} />
					</SectionCard>
				{/if}
			</div>
		</ScrollArea>
	{/if}
</div>
