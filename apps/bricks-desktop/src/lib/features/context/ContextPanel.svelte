<script lang="ts">
	// Right panel: pending approvals, the project environment and the context
	// occupation of the shown conversation. Its sub-agents are in the thread.
	import { app } from '$lib/state/app.svelte';
	import { ScrollArea } from '$lib/components/ui/scroll-area';
	import EmptyState from '$lib/layout/EmptyState.svelte';
	import ContextCard from './ContextCard.svelte';
	import PendingApprovals from './PendingApprovals.svelte';
	import ProjectEnvironment from './ProjectEnvironment.svelte';
	import FolderOpenIcon from '@lucide/svelte/icons/folder-open';

	const project = $derived(app.currentProject);
	const conversation = $derived(app.current);
	const approvals = $derived(conversation?.pendingApprovals ?? []);

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
				{#if conversation}
					<PendingApprovals {approvals} />
				{/if}

				<ProjectEnvironment {project} {conversation} />

				{#if conversation}
					<ContextCard {conversation} />
				{/if}
			</div>
		</ScrollArea>
	{/if}
</div>
