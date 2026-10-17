<script lang="ts">
	// Short editor of one effort profile, inside the model form. Nothing is written here: the whole
	// profile list is saved with the model. These are the client checks; the engine refuses the rest
	// (protected fields, pointers) and its refusal is shown when the model is saved.
	import { untrack } from 'svelte';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Label } from '$lib/components/ui/label';
	import { Textarea } from '$lib/components/ui/textarea';
	import * as Collapsible from '$lib/components/ui/collapsible';
	import ChevronDown from '@lucide/svelte/icons/chevron-down';
	import Trash from '@lucide/svelte/icons/trash';
	import { parseProfileDraft, type ProfileDraft, type ProfileErrors } from './models';

	let {
		draft,
		isNew,
		takenIds,
		onCommit,
		onCancel,
		onRemove
	}: {
		draft: ProfileDraft;
		isNew: boolean;
		/** Ids of the other profiles of the model. */
		takenIds: string[];
		/** Hands the checked draft back to the form. */
		onCommit: (draft: ProfileDraft) => void;
		onCancel: () => void;
		/** Only for a profile that already exists in the form. */
		onRemove?: () => void;
	} = $props();

	const uid = $props.id();
	const start = untrack(() => draft);
	let id = $state(start.id);
	let label = $state(start.label);
	let parameters = $state(start.parameters);
	let remove = $state(start.remove);
	let advancedOpen = $state(start.remove.trim() !== '');
	let attempted = $state(false);

	const checked = $derived(parseProfileDraft({ id, label, parameters, remove }, takenIds));
	const errors: ProfileErrors = $derived(checked.ok ? {} : checked.errors);
	const shown = $derived(attempted ? errors : {});

	function commit() {
		attempted = true;
		if (!checked.ok) {
			if (checked.errors.remove) advancedOpen = true;
			return;
		}
		onCommit({ id: id.trim(), label: label.trim(), parameters, remove });
	}

	/** Enter in a text field validates the profile; it must not submit the model form around it. */
	function onKeydown(event: KeyboardEvent) {
		if (event.key !== 'Enter' || !(event.target instanceof HTMLInputElement)) return;
		event.preventDefault();
		commit();
	}
</script>

<div
	role="group"
	aria-labelledby={`${uid}-title`}
	class="flex flex-col gap-3 rounded-lg border border-border bg-muted/40 p-3"
>
	<p id={`${uid}-title`} class="text-sm font-medium text-foreground">
		{isNew ? 'Nouveau profil d’effort' : `Profil « ${start.id} »`}
	</p>

	<div class="grid gap-3 sm:grid-cols-2">
		<div class="flex flex-col gap-1.5">
			<Label for={`${uid}-id`}>Identifiant</Label>
			<Input
				id={`${uid}-id`}
				bind:value={id}
				maxlength={60}
				autocomplete="off"
				spellcheck={false}
				onkeydown={onKeydown}
				class="font-mono text-sm"
				aria-invalid={shown.id ? 'true' : undefined}
				aria-describedby={shown.id ? `${uid}-id-error` : undefined}
			/>
			{#if shown.id}
				<p id={`${uid}-id-error`} role="alert" class="text-xs text-destructive">{shown.id}</p>
			{/if}
		</div>

		<div class="flex flex-col gap-1.5">
			<Label for={`${uid}-label`}>Libellé <span class="text-muted-foreground">(facultatif)</span></Label>
			<Input
				id={`${uid}-label`}
				bind:value={label}
				maxlength={60}
				autocomplete="off"
				placeholder={id.trim()}
				onkeydown={onKeydown}
			/>
		</div>
	</div>

	<div class="flex flex-col gap-1.5">
		<Label for={`${uid}-parameters`}>Paramètres (objet JSON)</Label>
		<Textarea
			id={`${uid}-parameters`}
			bind:value={parameters}
			rows={4}
			spellcheck={false}
			class="font-mono text-xs"
			aria-invalid={shown.parameters ? 'true' : undefined}
			aria-describedby={shown.parameters ? `${uid}-parameters-error` : `${uid}-parameters-help`}
		/>
		{#if shown.parameters}
			<p id={`${uid}-parameters-error`} role="alert" class="text-xs text-destructive">{shown.parameters}</p>
		{:else}
			<p id={`${uid}-parameters-help`} class="text-xs text-muted-foreground">
				Ajoutés à la requête du modèle. Le moteur refuse les champs de structure.
			</p>
		{/if}
	</div>

	<Collapsible.Root bind:open={advancedOpen}>
		<Collapsible.Trigger
			type="button"
			class="inline-flex items-center gap-1 self-start text-xs text-muted-foreground hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
		>
			<ChevronDown class={`size-3.5 transition-transform ${advancedOpen ? 'rotate-180' : ''}`} aria-hidden="true" />
			Chemins à retirer
		</Collapsible.Trigger>
		<Collapsible.Content class="flex flex-col gap-1.5 pt-2">
			<Label for={`${uid}-remove`}>Un chemin JSON Pointer par ligne</Label>
			<Textarea
				id={`${uid}-remove`}
				bind:value={remove}
				rows={2}
				spellcheck={false}
				placeholder="/champ/sous-champ"
				class="font-mono text-xs"
				aria-invalid={shown.remove ? 'true' : undefined}
				aria-describedby={shown.remove ? `${uid}-remove-error` : undefined}
			/>
			{#if shown.remove}
				<p id={`${uid}-remove-error`} role="alert" class="text-xs text-destructive">{shown.remove}</p>
			{/if}
		</Collapsible.Content>
	</Collapsible.Root>

	<div class="flex flex-wrap items-center justify-between gap-2">
		{#if onRemove}
			<Button
				type="button"
				variant="ghost"
				size="sm"
				class="text-destructive hover:bg-destructive/10 hover:text-destructive"
				onclick={onRemove}
			>
				<Trash aria-hidden="true" />
				Retirer le profil
			</Button>
		{:else}
			<span></span>
		{/if}
		<div class="flex gap-2">
			<Button type="button" variant="outline" size="sm" onclick={onCancel}>Annuler</Button>
			<Button type="button" size="sm" onclick={commit}>{isNew ? 'Ajouter le profil' : 'Valider le profil'}</Button>
		</div>
	</div>
</div>
