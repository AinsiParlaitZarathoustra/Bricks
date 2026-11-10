<script lang="ts">
	// Left panel: projects with their conversations, the conversations without a project, and search.
	// Entry point mounted by App.svelte with no props. All state lives in `app`.
	import { t } from '$lib/i18n/i18n.svelte';
	import FolderPlus from '@lucide/svelte/icons/folder-plus';
	import Settings from '@lucide/svelte/icons/settings';
	import { Button } from '$lib/components/ui/button';
	import { ScrollArea } from '$lib/components/ui/scroll-area';
	import { app } from '$lib/state/app.svelte';
	import { ui } from '$lib/state/ui.svelte';
	import ConversationList from './ConversationList.svelte';
	import NewConversationButton from './NewConversationButton.svelte';
	import ProjectList from './ProjectList.svelte';
	import SearchDialog from './SearchDialog.svelte';

	let adding = $state(false);

	const chats = $derived(app.chatsProject);

	async function addProject() {
		if (adding) return;
		adding = true;
		try {
			await app.addProject();
		} catch (error) {
			app.fail(error);
		} finally {
			adding = false;
		}
	}
</script>

<aside
	aria-label={t('navigation.projectsAndConversations')}
	class="flex h-full min-h-0 flex-col bg-sidebar text-sidebar-foreground"
>
	<div class="relative flex flex-col gap-0.5 px-2 pt-7 pb-3">
		<div class="absolute top-1 right-2">
			<SearchDialog />
		</div>
		<NewConversationButton class="w-full justify-start px-2 text-ui" />
	</div>

	<ScrollArea class="min-h-0 flex-1">
		<div class="flex flex-col gap-4 pb-3">
			{#if app.pinnedSessions.length > 0}
				<section class="flex flex-col gap-1 px-2">
					<h2 class="px-2 text-xs font-medium text-muted-foreground">{t('navigation.pinned')}</h2>
					<ConversationList sessions={app.pinnedSessions} label={t('navigation.pinned')} />
				</section>
			{/if}

			<section class="flex flex-col gap-1 px-2">
				<div class="flex h-6 items-center justify-between pr-0.5 pl-2">
					<h2 class="text-xs font-medium text-muted-foreground">{t('navigation.projects')}</h2>
					<Button
						variant="ghost"
						size="icon-xs"
						class="text-muted-foreground hover:text-foreground"
						aria-label={t('navigation.newProject')}
						title={t('navigation.newProject')}
						onclick={addProject}
						disabled={adding}
					>
						<FolderPlus class="size-3.5" aria-hidden="true" />
					</Button>
				</div>
				{#if app.folders.length > 0}
					<ProjectList />
				{/if}
			</section>

			{#if chats && app.sessionsOf(chats.id).some((s) => !app.isPinned(s.id))}
				<section class="flex flex-col gap-1 px-2">
					<h2 class="px-2 text-xs font-medium text-muted-foreground">{t('navigation.conversations')}</h2>
					<ConversationList projectId={chats.id} label={t('navigation.conversations')} />
				</section>
			{/if}
		</div>
	</ScrollArea>

	<div class="p-2">
		<Button
			variant="ghost"
			class="w-full justify-start text-ui"
			aria-label={t('common.bricksSettings')}
			onclick={() => (ui.settingsOpen = true)}
		>
			<Settings class="size-4" aria-hidden="true" />
			{t('navigation.settings')}
		</Button>
	</div>
</aside>
