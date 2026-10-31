<script lang="ts">
	import { Badge } from '$lib/components/ui/badge';
	import SettingsGroup from './SettingsGroup.svelte';

	/** Planned options. Not implemented yet: shown as placeholders, never as working controls. */
	const PLANNED = [
		{ title: 'Export des conversations' },
		{ title: 'Journal technique' }
	];
</script>

<div class="flex flex-col gap-6">
	<SettingsGroup title="Options prévues">
		<ul class="flex flex-col divide-y divide-border">
			{#each PLANNED as item (item.title)}
				<li class="flex items-center justify-between gap-4 p-3">
					<span class="min-w-0 text-sm font-medium text-muted-foreground">{item.title}</span>
					<Badge variant="outline" class="shrink-0">À venir</Badge>
				</li>
			{/each}
		</ul>
	</SettingsGroup>
</div>
