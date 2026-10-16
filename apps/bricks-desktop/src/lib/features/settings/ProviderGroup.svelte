<script lang="ts">
	// One provider and its models, in a single discreet container: a header row, then one line per model.
	import Pencil from '@lucide/svelte/icons/pencil';
	import Trash from '@lucide/svelte/icons/trash';
	import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
	import { Button } from '$lib/components/ui/button';
	import * as Tooltip from '$lib/components/ui/tooltip';
	import type { ModelView } from '$lib/ipc/bindings/ModelView';
	import type { ProviderView } from '$lib/ipc/bindings/ProviderView';
	import ModelRow from './ModelRow.svelte';
	import { displayName } from './models';

	let {
		provider,
		onEditProvider,
		onDeleteProvider,
		onEditModel,
		onDeleteModel
	}: {
		provider: ProviderView;
		onEditProvider: () => void;
		onDeleteProvider: () => void;
		onEditModel: (model: ModelView) => void;
		onDeleteModel: (model: ModelView) => void;
	} = $props();

	const uid = $props.id();
	const title = $derived(displayName(provider.name, provider.id));
</script>

<section class="flex flex-col rounded-lg border border-border" aria-labelledby={`${uid}-name`}>
	<header class="flex items-center gap-2 px-3.5 py-2.5">
		<div class="flex min-w-0 flex-1 flex-col">
			<h4 id={`${uid}-name`} class="truncate text-sm font-medium text-foreground">{title}</h4>
			<p class="flex min-w-0 items-center gap-2 text-xs text-muted-foreground">
				<span class="truncate font-mono">{provider.endpoint}</span>
				{#if !provider.has_credentials}
					<span class="inline-flex shrink-0 items-center gap-1 text-warning">
						<TriangleAlert class="size-3.5" aria-hidden="true" />
						Identifiants absents
					</span>
				{/if}
			</p>
		</div>

		<Tooltip.Root>
			<Tooltip.Trigger>
				{#snippet child({ props })}
					<Button
						{...props}
						variant="ghost"
						size="icon-sm"
						class="shrink-0 text-muted-foreground"
						aria-label={`Modifier le fournisseur ${title}`}
						onclick={onEditProvider}
					>
						<Pencil aria-hidden="true" />
					</Button>
				{/snippet}
			</Tooltip.Trigger>
			<Tooltip.Content>Modifier le fournisseur</Tooltip.Content>
		</Tooltip.Root>

		<Tooltip.Root>
			<Tooltip.Trigger>
				{#snippet child({ props })}
					<Button
						{...props}
						variant="ghost"
						size="icon-sm"
						class="shrink-0 text-destructive hover:bg-destructive/10 hover:text-destructive"
						aria-label={`Supprimer le fournisseur ${title}`}
						onclick={onDeleteProvider}
					>
						<Trash aria-hidden="true" />
					</Button>
				{/snippet}
			</Tooltip.Trigger>
			<Tooltip.Content>Supprimer le fournisseur</Tooltip.Content>
		</Tooltip.Root>
	</header>

	{#if provider.models.length === 0}
		<p class="border-t border-border px-3.5 py-2.5 text-sm text-muted-foreground">Aucun modèle pour ce fournisseur.</p>
	{:else}
		<ul class="flex flex-col gap-0.5 border-t border-border py-1" aria-label={`Modèles de ${title}`}>
			{#each provider.models as model (model.id)}
				<ModelRow
					{model}
					onEdit={() => onEditModel(model)}
					onDelete={() => onDeleteModel(model)}
				/>
			{/each}
		</ul>
	{/if}
</section>
