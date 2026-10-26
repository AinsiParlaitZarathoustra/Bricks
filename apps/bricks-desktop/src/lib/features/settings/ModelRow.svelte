<script lang="ts">
	// One model inside a provider container: its name, edit and delete. The name expands the details.
	import Pencil from '@lucide/svelte/icons/pencil';
	import Trash from '@lucide/svelte/icons/trash';
	import { Button } from '$lib/components/ui/button';
	import * as Tooltip from '$lib/components/ui/tooltip';
	import type { ModelView } from '$lib/ipc/bindings/ModelView';
	import { formatModalities, formatPrice, formatTokens } from './format';
	import { displayName, profileLabel } from './models';

	let {
		model,
		onEdit,
		onDelete
	}: {
		model: ModelView;
		onEdit: () => void;
		onDelete: () => void;
	} = $props();

	const uid = $props.id();
	const detailId = `${uid}-detail`;
	const title = $derived(displayName(model.name, model.id));
	let expanded = $state(false);
</script>

<li class="flex flex-col">
	<div class="flex items-center gap-1 px-1.5">
		<button
			type="button"
			class="min-w-0 flex-1 truncate rounded-md px-2 py-1.5 text-left text-ui text-foreground hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
			aria-expanded={expanded}
			aria-controls={expanded ? detailId : undefined}
			onclick={() => (expanded = !expanded)}
		>
			{title}
		</button>

		<Tooltip.Root>
			<Tooltip.Trigger>
				{#snippet child({ props })}
					<Button
						{...props}
						variant="ghost"
						size="icon-sm"
						class="shrink-0 text-muted-foreground"
						aria-label={`Modifier le modèle ${title}`}
						onclick={onEdit}
					>
						<Pencil aria-hidden="true" />
					</Button>
				{/snippet}
			</Tooltip.Trigger>
			<Tooltip.Content>Modifier le modèle</Tooltip.Content>
		</Tooltip.Root>

		<Tooltip.Root>
			<Tooltip.Trigger>
				{#snippet child({ props })}
					<Button
						{...props}
						variant="ghost"
						size="icon-sm"
						class="shrink-0 text-destructive hover:bg-destructive/10 hover:text-destructive"
						aria-label={`Supprimer le modèle ${title}`}
						onclick={onDelete}
					>
						<Trash aria-hidden="true" />
					</Button>
				{/snippet}
			</Tooltip.Trigger>
			<Tooltip.Content>Supprimer le modèle</Tooltip.Content>
		</Tooltip.Root>
	</div>

	{#if expanded}
		<dl id={detailId} class="grid grid-cols-[auto_1fr] items-baseline gap-x-4 gap-y-1.5 px-3.5 pt-1 pb-3 text-xs">
			<dt class="text-muted-foreground">Sélection</dt>
			<dd class="font-mono break-all text-foreground">{model.selection}</dd>

			<dt class="text-muted-foreground">Nom API</dt>
			<dd class="font-mono break-all text-foreground">{model.api_model}</dd>

			<dt class="text-muted-foreground">Entrée maximale</dt>
			<dd class="tabular-nums text-foreground">{formatTokens(model.max_input_tokens)}</dd>

			<dt class="text-muted-foreground">Sortie maximale</dt>
			<dd class="tabular-nums text-foreground">{formatTokens(model.max_output_tokens)}</dd>

			<dt class="text-muted-foreground">Entrées acceptées</dt>
			<dd class="text-foreground">{formatModalities(model.input_modalities)}</dd>

			<dt class="text-muted-foreground">Profils d’effort</dt>
			<dd class="text-foreground">
				{#if model.profiles.length === 0}
					Aucun
				{:else}
					<ul class="flex flex-col gap-0.5">
						{#each model.profiles as profile (profile.id)}
							<li>
								{profileLabel(profile)}
								{#if profile.id === model.reasoning_default}
									<span class="text-muted-foreground">(par défaut)</span>
								{/if}
							</li>
						{/each}
					</ul>
				{/if}
			</dd>

			<dt class="text-muted-foreground">Coûts / M tokens</dt>
			<dd class="tabular-nums text-foreground">
				{#if model.pricing}
					Entrée {formatPrice(model.pricing.input)} · Sortie {formatPrice(model.pricing.output)} · Cache {formatPrice(model.pricing.cache_read)}
				{:else}
					Non renseignés
				{/if}
			</dd>
		</dl>
	{/if}
</li>
