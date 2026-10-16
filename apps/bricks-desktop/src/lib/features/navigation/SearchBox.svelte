<script lang="ts">
	import Search from '@lucide/svelte/icons/search';
	import { Input } from '$lib/components/ui/input';
	import { Label } from '$lib/components/ui/label';
	import { Switch } from '$lib/components/ui/switch';

	interface Props {
		/** The text being searched. Empty when no search is active. */
		query?: string;
		/** Search every project instead of the current one. */
		allProjects?: boolean;
	}

	let { query = $bindable(''), allProjects = $bindable(false) }: Props = $props();

	const uid = $props.id();

	function onKeydown(event: KeyboardEvent) {
		// Escape clears the search first; a second Escape reaches the rest of the app.
		if (event.key === 'Escape' && query) {
			event.preventDefault();
			event.stopPropagation();
			query = '';
		}
	}
</script>

<div class="flex flex-col gap-2">
	<div class="relative">
		<Search class="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" aria-hidden="true" />
		<Input
			type="search"
			bind:value={query}
			placeholder="Rechercher…"
			aria-label="Rechercher dans les conversations (titres et messages)"
			autocomplete="off"
			spellcheck={false}
			class="h-8 pl-8 text-sm"
			onkeydown={onKeydown}
		/>
	</div>
	<div class="flex items-center gap-2">
		<Switch id={`${uid}-all`} bind:checked={allProjects} />
		<Label for={`${uid}-all`} class="text-xs font-normal text-muted-foreground">Tous les projets</Label>
	</div>
</div>
