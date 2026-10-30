<script lang="ts">
	// Form of a model: a new one (`model` null) or an existing one, from its values when opened.
	// Cancel discards.
	// Only counts above zero are checked here; the engine checks the rest. The effort profiles are
	// edited as a whole list and saved with the model.
	import { untrack } from 'svelte';
	import * as Dialog from '$lib/components/ui/dialog';
	import * as Select from '$lib/components/ui/select';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Label } from '$lib/components/ui/label';
	import { Switch } from '$lib/components/ui/switch';
	import type { ModelPatch } from '$lib/ipc/bindings/ModelPatch';
	import type { ModelView } from '$lib/ipc/bindings/ModelView';
	import Plus from '@lucide/svelte/icons/plus';
	import { MODALITY_LABELS } from './format';
	import ReasoningProfileEditor from './ReasoningProfileEditor.svelte';
	import {
		DOCUMENT_TYPE_PRESETS,
		MODALITY_OPTIONS,
		buildModelCreatePatch,
		buildModelPatch,
		documentTypes,
		displayName,
		draftLabel,
		hasFormErrors,
		isEmptyModelPatch,
		modelForm,
		newModelForm,
		newProfileDraft,
		profilesPatch,
		validateModelForm,
		validateNewId,
		type ModelForm,
		type ProfileDraft,
		type WriteOutcome
	} from './models';

	let {
		model,
		takenIds = [],
		notice = null,
		onSave,
		onClose
	}: {
		/** `null`: a new model. */
		model: ModelView | null;
		/** Ids of the provider's models (a new model needs its own). */
		takenIds?: readonly string[];
		/** Set when the dialog was reopened after the file changed elsewhere. */
		notice?: string | null;
		/** Writes the patch (every field for a new model, else only the changed ones). */
		onSave: (patch: ModelPatch, id: string) => Promise<WriteOutcome>;
		/** The dialog must go: closed by the user, or after a successful save. */
		onClose: () => void;
	} = $props();

	/** Select values that are not profile ids. */
	const NO_PROFILE = '__none__';
	const PENDING = '__pending__';

	const uid = $props.id();
	const original = untrack(() => model);
	let form = $state<ModelForm>(original ? modelForm(original) : newModelForm());
	let id = $state('');
	let attempted = $state(false);
	let saving = $state(false);
	let error = $state<string | null>(null);
	/** The profile being added (`index` null) or edited. One at a time. */
	let editing = $state<{ index: number | null; draft: ProfileDraft } | null>(null);

	const errors = $derived(validateModelForm(form));
	const idError = $derived(original ? undefined : validateNewId(id, takenIds));

	function togglePreset(value: string) {
		const types = documentTypes(form.documentTypes);
		const next = types.includes(value) ? types.filter((t) => t !== value) : [...types, value];
		form.documentTypes = next.join(', ');
	}
	/** Field errors show once the user tried to save. */
	const shown = $derived(attempted ? errors : {});

	const defaultValue = $derived(form.defaultPending ? PENDING : (form.reasoningDefault ?? NO_PROFILE));
	const defaultText = $derived.by(() => {
		if (form.defaultPending) return 'Choisir un profil';
		if (form.reasoningDefault === null) return 'Aucun';
		const profile = form.profiles.find((p) => p.id === form.reasoningDefault);
		return profile ? draftLabel(profile) : (form.reasoningDefault ?? '');
	});

	/** Ids of the profiles other than the one at `index` (all of them for a new profile). */
	function takenIdsFor(index: number | null): string[] {
		return form.profiles.filter((_, i) => i !== index).map((p) => p.id.trim());
	}

	function openNew() {
		editing = { index: null, draft: newProfileDraft() };
	}

	function openEdit(index: number) {
		editing = { index, draft: { ...form.profiles[index] } };
	}

	function commitProfile(next: ProfileDraft) {
		const current = editing;
		if (!current) return;
		if (current.index === null) {
			form.profiles = [...form.profiles, next];
		} else {
			const before = form.profiles[current.index];
			form.profiles = form.profiles.map((p, i) => (i === current.index ? next : p));
			// A renamed default is no longer the same profile: the user chooses again.
			if (form.reasoningDefault === before.id && before.id !== next.id) {
				form.reasoningDefault = null;
				form.defaultPending = true;
			}
		}
		editing = null;
	}

	function removeProfile(index: number) {
		const removed = form.profiles[index];
		form.profiles = form.profiles.filter((_, i) => i !== index);
		if (form.reasoningDefault === removed.id) {
			form.reasoningDefault = null;
			form.defaultPending = true;
		}
		editing = null;
	}

	function chooseDefault(next: string | undefined) {
		if (next === undefined || next === PENDING) return;
		form.defaultPending = false;
		form.reasoningDefault = next === NO_PROFILE ? null : next;
	}

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		if (saving) return;
		attempted = true;
		if (editing) {
			error = 'Validez ou annulez le profil en cours d’édition avant d’enregistrer.';
			return;
		}
		if (hasFormErrors(errors) || idError) return;
		const profiles = profilesPatch(original?.profiles ?? [], form.profiles);
		if (!profiles.ok) {
			error = profiles.message;
			return;
		}
		const patch = original
			? buildModelPatch(original, form, profiles.edits)
			: buildModelCreatePatch(form, profiles.edits);
		if (original && isEmptyModelPatch(patch)) {
			onClose();
			return;
		}
		saving = true;
		error = null;
		const outcome = await onSave(patch, original ? original.id : id.trim());
		saving = false;
		if (outcome.ok) onClose();
		else error = outcome.message;
	}
</script>

<Dialog.Root open onOpenChange={(next) => (next ? undefined : onClose())}>
	<Dialog.Content class="max-h-[min(46rem,calc(100dvh-2rem))] overflow-y-auto sm:max-w-lg">
		<form onsubmit={submit} class="flex flex-col gap-5" novalidate>
			<Dialog.Header>
				<Dialog.Title>
					{original ? `Modifier le modèle ${displayName(original.name, original.id)}` : 'Ajouter un modèle'}
				</Dialog.Title>
			</Dialog.Header>

			{#if notice}
				<p role="alert" class="text-sm text-pretty text-destructive">{notice}</p>
			{/if}

			{#if !original}
				<div class="flex flex-col gap-2">
					<Label for={`${uid}-id`}>Identifiant</Label>
					<Input
						id={`${uid}-id`}
						bind:value={id}
						maxlength={64}
						autocomplete="off"
						spellcheck={false}
						class="font-mono text-sm"
						aria-invalid={attempted && idError ? 'true' : undefined}
						aria-describedby={attempted && idError ? `${uid}-id-error` : undefined}
					/>
					{#if attempted && idError}
						<p id={`${uid}-id-error`} role="alert" class="text-xs text-pretty text-destructive">{idError}</p>
					{/if}
				</div>
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
				<Label for={`${uid}-api-model`}>Nom API</Label>
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
				{#if shown.modalities}
					<p role="alert" class="text-xs text-destructive">{shown.modalities}</p>
				{/if}
			</fieldset>

			{#if form.modalities.includes('document')}
				<div class="flex flex-col gap-2">
					<Label for={`${uid}-documents`}>Types de fichiers acceptés</Label>
					<div class="flex flex-wrap items-center gap-1.5">
						{#each DOCUMENT_TYPE_PRESETS as preset (preset.value)}
							{@const active = documentTypes(form.documentTypes).includes(preset.value)}
							<button
								type="button"
								aria-pressed={active}
								class="inline-flex h-7 items-center rounded-full border px-2.5 text-xs hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none {active ? 'border-ring bg-muted text-foreground' : 'border-border text-muted-foreground'}"
								onclick={() => togglePreset(preset.value)}
							>
								{preset.label}
							</button>
						{/each}
					</div>
					<Input
						id={`${uid}-documents`}
						bind:value={form.documentTypes}
						autocomplete="off"
						spellcheck={false}
						placeholder="application/pdf, text/*"
						class="font-mono text-sm"
						aria-invalid={shown.documentTypes ? 'true' : undefined}
						aria-describedby={shown.documentTypes ? `${uid}-documents-error` : undefined}
					/>
					{#if shown.documentTypes}
						<p id={`${uid}-documents-error`} role="alert" class="text-xs text-pretty text-destructive">{shown.documentTypes}</p>
					{/if}
				</div>
			{/if}

			<fieldset class="flex flex-col gap-2">
				<legend class="mb-2 text-sm leading-none font-medium">Coûts (USD par million de tokens)</legend>
				<div class="grid gap-4 sm:grid-cols-3">
					{#each [{ key: 'priceInput', label: 'Entrée' }, { key: 'priceOutput', label: 'Sortie' }, { key: 'priceCache', label: 'Cache' }] as const as field (field.key)}
						<div class="flex flex-col gap-2">
							<Label for={`${uid}-${field.key}`}>{field.label}</Label>
							<Input
								id={`${uid}-${field.key}`}
								bind:value={form[field.key]}
								inputmode="decimal"
								autocomplete="off"
								class="tabular-nums"
								aria-invalid={shown[field.key] ? 'true' : undefined}
								aria-describedby={shown[field.key] ? `${uid}-${field.key}-error` : undefined}
							/>
							{#if shown[field.key]}
								<p id={`${uid}-${field.key}-error`} role="alert" class="text-xs text-pretty text-destructive">{shown[field.key]}</p>
							{/if}
						</div>
					{/each}
				</div>
			</fieldset>

			<div class="flex flex-col gap-2">
				<span id={`${uid}-profiles-label`} class="text-sm leading-none font-medium">Profils d’effort</span>
				<div class="flex flex-wrap items-center gap-1.5" role="group" aria-labelledby={`${uid}-profiles-label`}>
					{#each form.profiles as profile, index (profile.id)}
						<button
							type="button"
							class="inline-flex h-7 max-w-full items-center gap-1.5 rounded-full border bg-background px-2.5 text-xs text-foreground hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none {editing?.index === index ? 'border-ring' : 'border-border'}"
							onclick={() => openEdit(index)}
						>
							<span class="truncate">{draftLabel(profile)}</span>
							{#if form.reasoningDefault === profile.id}
								<span class="shrink-0 text-muted-foreground">(défaut)</span>
							{/if}
						</button>
					{/each}
					<Button
						type="button"
						variant="outline"
						size="icon-xs"
						class="rounded-full"
						aria-label="Ajouter un profil d’effort"
						onclick={openNew}
					>
						<Plus aria-hidden="true" />
					</Button>
				</div>
			</div>

			{#if editing}
				{@const current = editing}
				{@const index = current.index}
				{#key current}
					<ReasoningProfileEditor
						draft={current.draft}
						isNew={index === null}
						takenIds={takenIdsFor(index)}
						onCommit={commitProfile}
						onCancel={() => (editing = null)}
						onRemove={index === null ? undefined : () => removeProfile(index)}
					/>
				{/key}
			{/if}

			<div class="flex flex-col gap-2">
				<Label for={`${uid}-profile`}>Profil d’effort par défaut</Label>
				<Select.Root type="single" value={defaultValue} onValueChange={chooseDefault}>
					<Select.Trigger
						id={`${uid}-profile`}
						class="w-full"
						aria-invalid={shown.defaultProfile ? 'true' : undefined}
						aria-describedby={shown.defaultProfile ? `${uid}-profile-error` : undefined}
					>
						<span class="truncate">{defaultText}</span>
					</Select.Trigger>
					<Select.Content>
						{#if form.defaultPending}
							<Select.Item value={PENDING} label="Choisir un profil" disabled>Choisir un profil</Select.Item>
						{/if}
						<Select.Item value={NO_PROFILE} label="Aucun">Aucun</Select.Item>
						{#each form.profiles as profile (profile.id)}
							<Select.Item value={profile.id} label={draftLabel(profile)}>{draftLabel(profile)}</Select.Item>
						{/each}
					</Select.Content>
				</Select.Root>
				{#if shown.defaultProfile}
					<p id={`${uid}-profile-error`} role="alert" class="text-xs text-destructive">{shown.defaultProfile}</p>
				{/if}
			</div>

			<label class="flex items-center justify-between gap-3 text-sm">
				Appels d’outils asynchrones
				<Switch bind:checked={form.asyncToolCalls} />
			</label>

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
