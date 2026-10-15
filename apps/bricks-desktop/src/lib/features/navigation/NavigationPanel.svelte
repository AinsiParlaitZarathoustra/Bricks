<script lang="ts">
	// Left panel: projects, conversations of the current project, and search.
	// Entry point mounted by App.svelte with no props. All state lives in `app`.
	import { FolderPlus } from '@lucide/svelte';
	import { Button } from '$lib/components/ui/button';
	import { ScrollArea } from '$lib/components/ui/scroll-area';
	import EmptyState from '$lib/layout/EmptyState.svelte';
	import { app } from '$lib/state/app.svelte';
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
	const project = $derived(app.currentProject);

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
		<NewConversationButton class="w-full justify-start" />
		<Button variant="ghost" class="w-full justify-start" onclick={addProject} disabled={adding}>
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
				<section class="flex flex-col gap-1 px-2">
					<h2 class="px-2 text-xs font-medium text-muted-foreground">Projets</h2>
					{#if app.projects.length === 0}
						<EmptyState
							title="Aucun projet"
							description="Un projet est un dossier de votre ordinateur. Ajoutez-en un pour démarrer une conversation."
						>
							{#snippet action()}
								<Button size="sm" onclick={addProject} disabled={adding}>
									<FolderPlus class="size-4" aria-hidden="true" />
									Nouveau projet
								</Button>
							{/snippet}
						</EmptyState>
					{:else}
						<ProjectList />
					{/if}
				</section>

				{#if app.projects.length > 0}
					<section class="flex flex-col gap-1 px-2">
						<h2 class="px-2 text-xs font-medium text-muted-foreground">Conversations</h2>
						{#if !project}
							<p class="px-2 py-1 text-sm text-muted-foreground">
								Sélectionnez un projet pour voir ses conversations.
							</p>
						{:else if app.sessions.length === 0}
							<EmptyState
								title="Aucune conversation"
								description={`Démarrez une conversation dans « ${project.name} ».`}
							>
								{#snippet action()}
									<NewConversationButton size="sm" />
								{/snippet}
							</EmptyState>
						{:else}
							<ConversationList />
						{/if}
					</section>
				{/if}
			{/if}
		</div>
	</ScrollArea>
</aside>
