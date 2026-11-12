<script lang="ts">
	// Right panel: pending approvals, the project environment and the context
	// occupation of the shown conversation. Its sub-agents are in the thread.
	import { t } from '$lib/i18n/i18n.svelte';
	import { app } from '$lib/state/app.svelte';
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
		viewport.setAttribute('aria-label', t('context.projectContext'));
	});
</script>

<div class="h-full">
	{#if !project}
		<EmptyState title={t('common.noOpenProject')} icon={FolderOpenIcon} />
	{:else}
		<div bind:this={viewport} class="scrollbar-subtle h-full overflow-y-auto overscroll-contain">
			<div class="flex flex-col gap-3 px-3 pt-5 pb-3">
				{#if conversation}
					<PendingApprovals {approvals} />
				{/if}

				<!-- A conversation without a project has no project folder to show. -->
				{#if !project.chats}
					<ProjectEnvironment {project} {conversation} />
				{/if}

				{#if conversation}
					<ContextCard {conversation} />
				{/if}
			</div>
		</div>
	{/if}
</div>
