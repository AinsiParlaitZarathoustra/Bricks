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
	import SearchBox from './SearchBox.svelte';
	import SearchResults from './SearchResults.svelte';

	let query = $state('');
	let allProjects = $state(false);
	let adding = $state(false);

	/** While the query is not empty, the results replace the project and conversation lists. */
	const searching = $derived(query.trim().length > 0);
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
	<div class="flex flex-col gap-1.5 p-3 pb-2">
		<NewConversationButton class="w-full justify-start text-[13px]" />
		<Button variant="ghost" class="w-full justify-start text-[13px]" onclick={addProject} disabled={adding}>
			<FolderPlus class="size-4" aria-hidden="true" />
			Nouveau projet
		</Button>
	</div>

	<div class="px-3 pb-3">
		<SearchBox bind:query bind:allProjects />
	</div>

	<ScrollArea class="min-h-0 flex-1">
		<div class="flex flex-col gap-4 pb-3">
			{#if searching}
				<SearchResults bind:query {allProjects} />
			{:else}
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
			{/if}
		</div>
	</ScrollArea>

	<div class="border-t border-sidebar-border/50 p-2">
		<Button
			variant="ghost"
			class="w-full justify-start text-[13px]"
			aria-label="Paramètres de Bricks"
			onclick={() => (ui.settingsOpen = true)}
		>
			<Settings class="size-4" aria-hidden="true" />
			Paramètres
		</Button>
	</div>
</aside>
