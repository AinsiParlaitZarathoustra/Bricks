<script lang="ts">
	import { Badge } from '$lib/components/ui/badge';
	import SettingsGroup from './SettingsGroup.svelte';

	/** Planned options. Not implemented yet: shown as placeholders, never as working controls. */
	const PLANNED = [
		{
			title: 'Export des conversations',
			description: 'Enregistrer une conversation dans un fichier lisible.'
		},
		{
			title: 'Journal technique',
			description: 'Consulter les journaux internes pour diagnostiquer un problème.'
		}
	];
</script>

<div class="flex flex-col gap-6">
	<p class="text-sm text-pretty text-muted-foreground">
		Ces options ne sont pas encore disponibles. Elles apparaîtront ici lorsqu’elles seront prêtes.
	</p>

	<SettingsGroup title="Options prévues">
		<ul class="flex flex-col divide-y divide-border">
			{#each PLANNED as item (item.title)}
				<li class="flex items-start justify-between gap-4 p-3">
					<div class="flex min-w-0 flex-col gap-0.5">
						<span class="text-sm font-medium text-muted-foreground">{item.title}</span>
						<span class="text-xs text-pretty text-muted-foreground">{item.description}</span>
					</div>
					<Badge variant="outline" class="shrink-0">À venir</Badge>
				</li>
			{/each}
		</ul>
	</SettingsGroup>
</div>
