<script lang="ts">
	// Right panel « Modifications »: what the conversation changed, what the
	// project's journal recorded, and the Git state of the project folder.
	// Owner: Haiku 7 (changes feature).
	import * as Tabs from '$lib/components/ui/tabs';
	import ConversationChanges from './ConversationChanges.svelte';
	import HistoryChanges from './HistoryChanges.svelte';
	import GitChanges from './GitChanges.svelte';

	let tab = $state('conversation');
</script>

<div class="flex h-full min-h-0 flex-col">
	<h2 class="px-3 pt-3 pb-2 text-sm font-medium">Modifications</h2>
	<Tabs.Root bind:value={tab} class="flex min-h-0 flex-1 flex-col gap-0">
		<Tabs.List class="mx-3 grid grid-cols-3">
			<Tabs.Trigger value="conversation" class="min-w-0 truncate text-xs">Conversation</Tabs.Trigger>
			<!-- The visible label is short for the 280 px panel; the full name stays
				accessible and still contains the visible word (WCAG 2.5.3). -->
			<Tabs.Trigger
				value="historique"
				class="min-w-0 truncate text-xs"
				aria-label="Historique du projet"
				title="Historique du projet"
			>
				Historique
			</Tabs.Trigger>
			<Tabs.Trigger value="git" class="min-w-0 truncate text-xs">Git</Tabs.Trigger>
		</Tabs.List>
		<Tabs.Content value="conversation" class="min-h-0 flex-1 overflow-y-auto px-3 pt-3 pb-3 outline-none">
			<ConversationChanges />
		</Tabs.Content>
		<Tabs.Content value="historique" class="min-h-0 flex-1 overflow-y-auto px-3 pt-3 pb-3 outline-none">
			<HistoryChanges />
		</Tabs.Content>
		<Tabs.Content value="git" class="min-h-0 flex-1 overflow-y-auto px-3 pt-3 pb-3 outline-none">
			<GitChanges />
		</Tabs.Content>
	</Tabs.Root>
</div>
