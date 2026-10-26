<script lang="ts">
	// The magnifier next to « Nouvelle conversation », and the search it opens: titles and
	// messages of every project. Escape closes it and gives focus back to the magnifier.
	import Search from '@lucide/svelte/icons/search';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import * as Dialog from '$lib/components/ui/dialog';
	import * as Tooltip from '$lib/components/ui/tooltip';
	import SearchResults from './SearchResults.svelte';

	const LABEL = 'Rechercher dans les conversations';

	let open = $state(false);
	let query = $state('');
	let trigger = $state<HTMLButtonElement | null>(null);

	// Set when a result was opened: focus then stays with the conversation.
	let opened = false;

	function onOpenChange(next: boolean) {
		if (!next) query = '';
	}

	function onCloseAutoFocus(event: Event) {
		event.preventDefault();
		if (!opened) trigger?.focus();
		opened = false;
	}

	function onResultOpened() {
		opened = true;
		open = false;
		query = '';
	}
</script>

<Tooltip.Root>
	<Tooltip.Trigger>
		{#snippet child({ props })}
			<Button
				{...props}
				bind:ref={trigger}
				variant="ghost"
				size="icon-sm"
				class="shrink-0 text-muted-foreground"
				aria-label={LABEL}
				aria-haspopup="dialog"
				aria-expanded={open}
				onclick={() => (open = true)}
			>
				<Search class="size-3.5" aria-hidden="true" />
			</Button>
		{/snippet}
	</Tooltip.Trigger>
	<Tooltip.Content side="bottom">{LABEL}</Tooltip.Content>
</Tooltip.Root>

<Dialog.Root bind:open {onOpenChange}>
	<Dialog.Content
		showCloseButton={false}
		class="top-[15vh] translate-y-0 gap-0 overflow-hidden p-0 sm:max-w-lg"
		{onCloseAutoFocus}
	>
		<Dialog.Title class="sr-only">{LABEL}</Dialog.Title>
		<div class="relative border-b border-border">
			<Search
				class="pointer-events-none absolute top-1/2 left-3.5 size-4 -translate-y-1/2 text-muted-foreground"
				aria-hidden="true"
			/>
			<Input
				type="search"
				bind:value={query}
				placeholder="Rechercher…"
				aria-label={`${LABEL} (titres et messages)`}
				autocomplete="off"
				spellcheck={false}
				class="h-11 rounded-none border-0 pl-10 text-ui shadow-none focus-visible:ring-0 dark:bg-transparent"
			/>
		</div>
		{#if query.trim()}
			<div class="max-h-[60vh] overflow-y-auto overscroll-contain pt-2">
				<SearchResults bind:query onopened={onResultOpened} />
			</div>
		{/if}
	</Dialog.Content>
</Dialog.Root>
