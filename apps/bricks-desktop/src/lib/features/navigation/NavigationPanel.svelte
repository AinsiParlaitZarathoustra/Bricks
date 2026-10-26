<script lang="ts">
	// Left panel: projects with their conversations, the conversations without a project, and search.
	// Entry point mounted by App.svelte with no props. All state lives in `app`.
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
	aria-label="Projets et conversations"
	class="flex h-full min-h-0 flex-col bg-sidebar text-sidebar-foreground"
>
	<div class="relative flex flex-col gap-0.5 px-2 pt-7 pb-3">
		<div class="absolute top-1 right-2">
			<SearchDialog />
		</div>
		<NewConversationButton class="w-full justify-start px-2 text-ui" />
		<Button variant="ghost" class="w-full justify-start px-2 text-ui" onclick={addProject} disabled={adding}>
			<FolderPlus class="size-4" aria-hidden="true" />
			Nouveau projet
		</Button>
	</div>

	<ScrollArea class="min-h-0 flex-1">
		<div class="flex flex-col gap-4 pb-3">
			{#if app.folders.length > 0}
				<section class="flex flex-col gap-1 px-2">
					<h2 class="px-2 text-xs font-medium text-muted-foreground">Projets</h2>
					<ProjectList />
				</section>
			{/if}

			{#if chats && app.sessionsOf(chats.id).length > 0}
				<section class="flex flex-col gap-1 px-2">
					<h2 class="px-2 text-xs font-medium text-muted-foreground">Conversations</h2>
					<ConversationList projectId={chats.id} label="Conversations" />
				</section>
			{/if}
		</div>
	</ScrollArea>

	<div class="border-t border-sidebar-border/50 p-2">
		<Button
			variant="ghost"
			class="w-full justify-start text-ui"
			aria-label="Paramètres de Bricks"
			onclick={() => (ui.settingsOpen = true)}
		>
			<Settings class="size-4" aria-hidden="true" />
			Paramètres
		</Button>
	</div>
</aside>
