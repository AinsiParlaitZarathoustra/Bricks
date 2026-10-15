<script lang="ts">
	// Right panel: project environment, context occupation, pending approvals
	// and the sub-agents of the shown conversation. Owner: Haiku 6 (Sprint 14).
	import { app } from '$lib/state/app.svelte';
	import { ScrollArea } from '$lib/components/ui/scroll-area';
	import EmptyState from '$lib/layout/EmptyState.svelte';
	import SectionCard from '$lib/layout/SectionCard.svelte';
	import AgentList from '$lib/features/agents/AgentList.svelte';
	import ContextCard from './ContextCard.svelte';
	import PendingApprovals from './PendingApprovals.svelte';
	import ProjectEnvironment from './ProjectEnvironment.svelte';
	import FolderOpenIcon from '@lucide/svelte/icons/folder-open';

	const project = $derived(app.currentProject);
	const conversation = $derived(app.current);
	const approvals = $derived(conversation?.pendingApprovals ?? []);
	const agentCount = $derived(conversation?.agents.size ?? 0);

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
		<EmptyState
			title="Aucun projet ouvert"
			description="Ajoutez un dossier de projet pour voir son environnement, son contexte et ses sous-agents."
			icon={FolderOpenIcon}
		/>
	{:else}
		<ScrollArea class="h-full" bind:viewportRef={viewport}>
			<div class="flex flex-col gap-3 p-3">
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
