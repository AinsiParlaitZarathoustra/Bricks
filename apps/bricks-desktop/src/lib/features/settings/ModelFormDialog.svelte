<script lang="ts">
	// Edit form of a model. Starts from the model as it was when opened; Cancel discards.
	// Only counts above zero are checked here; the engine checks the rest.
	import { untrack } from 'svelte';
	import * as Dialog from '$lib/components/ui/dialog';
	import * as Select from '$lib/components/ui/select';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Label } from '$lib/components/ui/label';
	import type { ModelPatch } from '$lib/ipc/bindings/ModelPatch';
	import type { ModelView } from '$lib/ipc/bindings/ModelView';
	import { MODALITY_LABELS } from './format';
	import {
		MODALITY_OPTIONS,
		buildModelPatch,
		displayName,
		profileLabel,
		hasFormErrors,
		isEmptyModelPatch,
		modelForm,
		validateModelForm,
		type ModelForm,
		type WriteOutcome
	} from './models';

	let {
		model,
		notice = null,
		onSave,
		onClose
	}: {
		model: ModelView;
		/** Set when the dialog was reopened after the file changed elsewhere. */
		notice?: string | null;
		/** Writes the patch (only the changed fields); resolves with the outcome. */
		onSave: (patch: ModelPatch) => Promise<WriteOutcome>;
		/** The dialog must go: closed by the user, or after a successful save. */
		onClose: () => void;
	} = $props();

	/** Sentinel of the « Aucun » profile choice (it maps to `null`). */
	const NO_PROFILE = '__none__';

	const uid = $props.id();
	const original = untrack(() => model);
	let form = $state<ModelForm>(modelForm(original));
	let attempted = $state(false);
	let saving = $state(false);
	let error = $state<string | null>(null);

	const errors = $derived(validateModelForm(form));
	/** Field errors show once the user tried to save. */
	const shown = $derived(attempted ? errors : {});

	const profileText = $derived.by(() => {
		const id = form.reasoningDefault;
		if (id === null) return 'Aucun';
		const profile = original.profiles.find((p) => p.id === id);
		return profile ? profileLabel(profile) : id;
	});

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		if (saving) return;
		attempted = true;
		if (hasFormErrors(errors)) return;
		const patch = buildModelPatch(original, form);
		if (isEmptyModelPatch(patch)) {
			onClose();
			return;
		}
		saving = true;
		error = null;
		const outcome = await onSave(patch);
		saving = false;
		if (outcome.ok) onClose();
		else error = outcome.message;
	}
</script>

<Dialog.Root open onOpenChange={(next) => (next ? undefined : onClose())}>
	<Dialog.Content class="max-h-[min(46rem,calc(100dvh-2rem))] overflow-y-auto sm:max-w-lg">
		<form onsubmit={submit} class="flex flex-col gap-5" novalidate>
			<Dialog.Header>
				<Dialog.Title>Modifier le modèle {displayName(original.name, original.id)}</Dialog.Title>
			</Dialog.Header>

			{#if notice}
				<p role="alert" class="text-sm text-pretty text-destructive">{notice}</p>
			{/if}

			<div class="flex flex-col gap-2">
				<Label for={`${uid}-name`}>Nom</Label>
				<Input
					id={`${uid}-name`}
					bind:value={form.name}
					maxlength={120}
					autocomplete="off"
					aria-invalid={shown.name ? 'true' : undefined}
					aria-describedby={shown.name ? `${uid}-name-error` : undefined}
				/>
				{#if shown.name}
					<p id={`${uid}-name-error`} role="alert" class="text-xs text-destructive">{shown.name}</p>
				{/if}
			</div>

			<div class="flex flex-col gap-2">
				<Label for={`${uid}-api-model`}>Modèle de l’API</Label>
				<Input
					id={`${uid}-api-model`}
					bind:value={form.apiModel}
					autocomplete="off"
					spellcheck={false}
					class="font-mono text-sm"
					aria-invalid={shown.apiModel ? 'true' : undefined}
					aria-describedby={shown.apiModel ? `${uid}-api-model-error` : undefined}
				/>
				{#if shown.apiModel}
					<p id={`${uid}-api-model-error`} role="alert" class="text-xs text-destructive">{shown.apiModel}</p>
				{/if}
			</div>

			<div class="grid gap-4 sm:grid-cols-2">
				<div class="flex flex-col gap-2">
					<Label for={`${uid}-max-input`}>Entrée maximale (tokens)</Label>
					<Input
						id={`${uid}-max-input`}
						bind:value={form.maxInput}
						inputmode="numeric"
						autocomplete="off"
						aria-invalid={shown.maxInput ? 'true' : undefined}
						aria-describedby={shown.maxInput ? `${uid}-max-input-error` : undefined}
					/>
					{#if shown.maxInput}
						<p id={`${uid}-max-input-error`} role="alert" class="text-xs text-destructive">{shown.maxInput}</p>
					{/if}
				</div>

				<div class="flex flex-col gap-2">
					<Label for={`${uid}-max-output`}>Sortie maximale (tokens)</Label>
					<Input
						id={`${uid}-max-output`}
						bind:value={form.maxOutput}
						inputmode="numeric"
						autocomplete="off"
						aria-invalid={shown.maxOutput ? 'true' : undefined}
						aria-describedby={shown.maxOutput ? `${uid}-max-output-error` : undefined}
					/>
					{#if shown.maxOutput}
						<p id={`${uid}-max-output-error`} role="alert" class="text-xs text-destructive">{shown.maxOutput}</p>
					{/if}
				</div>
			</div>

			<div class="flex flex-col gap-2">
				<Label for={`${uid}-context`}>Fenêtre de contexte (tokens)</Label>
				<Input
					id={`${uid}-context`}
					bind:value={form.contextWindow}
					inputmode="numeric"
					placeholder="Inconnue"
					autocomplete="off"
					aria-invalid={shown.contextWindow ? 'true' : undefined}
					aria-describedby={shown.contextWindow ? `${uid}-context-error` : `${uid}-context-help`}
				/>
				{#if shown.contextWindow}
					<p id={`${uid}-context-error`} role="alert" class="text-xs text-destructive">{shown.contextWindow}</p>
				{:else}
					<p id={`${uid}-context-help`} class="text-xs text-muted-foreground">Laissez vide si elle est inconnue.</p>
				{/if}
			</div>

			<fieldset class="flex flex-col gap-2">
				<legend class="mb-2 text-sm leading-none font-medium">Entrées acceptées</legend>
				<div class="grid grid-cols-2 gap-2 sm:grid-cols-3">
					{#each MODALITY_OPTIONS as modality (modality)}
						<label class="flex items-center gap-2 text-sm">
							<input
								type="checkbox"
								value={modality}
								bind:group={form.modalities}
								class="size-4 accent-primary"
							/>
							{MODALITY_LABELS[modality] ?? modality}
						</label>
					{/each}
				</div>
			</fieldset>

			<div class="flex flex-col gap-2">
				<Label for={`${uid}-profile`}>Profil d’effort par défaut</Label>
				{#if original.profiles.length === 0}
					<p class="text-sm text-muted-foreground">Aucun profil d’effort pour ce modèle.</p>
				{:else}
					<Select.Root
						type="single"
						value={form.reasoningDefault ?? NO_PROFILE}
						onValueChange={(next) => {
							if (next !== undefined) form.reasoningDefault = next === NO_PROFILE ? null : next;
						}}
					>
						<Select.Trigger id={`${uid}-profile`} class="w-full">
							<span class="truncate">{profileText}</span>
						</Select.Trigger>
						<Select.Content>
							<Select.Item value={NO_PROFILE} label="Aucun">Aucun</Select.Item>
							{#each original.profiles as profile (profile.id)}
								<Select.Item value={profile.id} label={profileLabel(profile)}>
									{profileLabel(profile)}
								</Select.Item>
							{/each}
						</Select.Content>
					</Select.Root>
				{/if}
			</div>

			{#if error}
				<p role="alert" class="text-sm text-pretty text-destructive">{error}</p>
			{/if}

			<Dialog.Footer>
				<Button type="button" variant="outline" onclick={onClose}>Annuler</Button>
				<Button type="submit" disabled={saving}>Enregistrer</Button>
			</Dialog.Footer>
		</form>
	</Dialog.Content>
</Dialog.Root>
