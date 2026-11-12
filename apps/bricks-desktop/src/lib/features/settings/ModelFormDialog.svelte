<script lang="ts">
	// Form of a model: a new one (`model` null) or an existing one, from its values when opened.
	// Cancel discards.
	// Only counts above zero are checked here; the engine checks the rest. The reasoning levels are
	// edited as a whole list and saved with the model: a level is a name, the first one applies
	// until another is chosen.
	import { t } from '$lib/i18n/i18n.svelte';
	import { untrack } from 'svelte';
	import * as Dialog from '$lib/components/ui/dialog';
	import { Button } from '$lib/components/ui/button';
	import * as ButtonGroup from '$lib/components/ui/button-group';
	import { Input } from '$lib/components/ui/input';
	import * as Field from '$lib/components/ui/field';
	import { Checkbox } from '$lib/components/ui/checkbox';
	import { Switch } from '$lib/components/ui/switch';
	import type { ModelPatch } from '$lib/ipc/bindings/ModelPatch';
	import type { ModelView } from '$lib/ipc/bindings/ModelView';
	import X from '@lucide/svelte/icons/x';
	import { MODALITY_LABELS } from './format';
	import {
		MODALITY_OPTIONS,
		PROFILE_ID_DUPLICATE,
		buildModelCreatePatch,
		buildModelPatch,
		displayName,
		draftLabel,
		hasFormErrors,
		isEmptyModelPatch,
		modelForm,
		newLevelDraft,
		newModelForm,
		profilesPatch,
		validateModelForm,
		validateNewId,
		type ModelForm,
		type WriteOutcome
	} from './models';

	let {
		model,
		protocol,
		takenIds = [],
		notice = null,
		onSave,
		onClose
	}: {
		/** `null`: a new model. */
		model: ModelView | null;
		/** The protocol the model speaks: a new level sends its effort in this protocol's field. */
		protocol: string;
		/** Ids of the provider's models (a new model needs its own). */
		takenIds?: readonly string[];
		/** Set when the dialog was reopened after the file changed elsewhere. */
		notice?: string | null;
		/** Writes the patch (every field for a new model, else only the changed ones). */
		onSave: (patch: ModelPatch, id: string) => Promise<WriteOutcome>;
		/** The dialog must go: closed by the user, or after a successful save. */
		onClose: () => void;
	} = $props();

	const uid = $props.id();
	const original = untrack(() => model);
	let form = $state<ModelForm>(original ? modelForm(original) : newModelForm());
	let id = $state('');
	let attempted = $state(false);
	let saving = $state(false);
	let error = $state<string | null>(null);
	/** The level being typed, added on Enter or with the button. */
	let level = $state('');
	let levelError = $state<string | null>(null);

	const errors = $derived(validateModelForm(form));
	const idError = $derived(original ? undefined : validateNewId(id, takenIds));

	/** Field errors show once the user tried to save. */
	const shown = $derived(attempted ? errors : {});

	function addLevel() {
		const name = level.trim();
		if (name === '') return;
		if (form.profiles.some((p) => p.id.trim() === name)) {
			levelError = PROFILE_ID_DUPLICATE();
			return;
		}
		form.profiles = [...form.profiles, newLevelDraft(name, protocol)];
		level = '';
		levelError = null;
	}

	function onLevelKey(event: KeyboardEvent) {
		if (event.key !== 'Enter' || event.isComposing) return;
		// Enter adds the level, it does not save the form.
		event.preventDefault();
		addLevel();
	}

	function removeLevel(index: number) {
		form.profiles = form.profiles.filter((_, i) => i !== index);
	}

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		if (saving) return;
		attempted = true;
		if (level.trim() !== '') addLevel();
		if (levelError) return;
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
					{original ? t('settings.editModel', { id: displayName(original.name, original.id) }) : t('settings.addAModel')}
				</Dialog.Title>
			</Dialog.Header>

			{#if notice}
				<p role="alert" class="text-sm text-pretty text-destructive">{notice}</p>
			{/if}

			{#if !original}
				<Field.Field data-invalid={attempted && idError ? 'true' : undefined}>
					<Field.Label for={`${uid}-id`}>{t('common.identifier')}</Field.Label>
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
						<Field.Error id={`${uid}-id-error`}>{idError}</Field.Error>
					{/if}
				</Field.Field>
			{/if}

			<Field.Field data-invalid={shown.name ? 'true' : undefined}>
				<Field.Label for={`${uid}-name`}>{t('settings.name')}</Field.Label>
				<Input
					id={`${uid}-name`}
					bind:value={form.name}
					maxlength={120}
					autocomplete="off"
					aria-invalid={shown.name ? 'true' : undefined}
					aria-describedby={shown.name ? `${uid}-name-error` : undefined}
				/>
				{#if shown.name}
					<Field.Error id={`${uid}-name-error`}>{shown.name}</Field.Error>
				{/if}
			</Field.Field>

			<Field.Field data-invalid={shown.apiModel ? 'true' : undefined}>
				<Field.Label for={`${uid}-api-model`}>{t('settings.apiName')}</Field.Label>
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
					<Field.Error id={`${uid}-api-model-error`}>{shown.apiModel}</Field.Error>
				{/if}
			</Field.Field>

			<div class="grid gap-4 sm:grid-cols-2">
				<Field.Field data-invalid={shown.maxInput ? 'true' : undefined}>
					<Field.Label for={`${uid}-max-input`}>{t('settings.maximumInputTokens')}</Field.Label>
					<Input
						id={`${uid}-max-input`}
						bind:value={form.maxInput}
						inputmode="numeric"
						autocomplete="off"
						aria-invalid={shown.maxInput ? 'true' : undefined}
						aria-describedby={shown.maxInput ? `${uid}-max-input-error` : undefined}
					/>
					{#if shown.maxInput}
						<Field.Error id={`${uid}-max-input-error`}>{shown.maxInput}</Field.Error>
					{/if}
				</Field.Field>

				<Field.Field data-invalid={shown.maxOutput ? 'true' : undefined}>
					<Field.Label for={`${uid}-max-output`}>{t('settings.maximumOutputTokens')}</Field.Label>
					<Input
						id={`${uid}-max-output`}
						bind:value={form.maxOutput}
						inputmode="numeric"
						autocomplete="off"
						aria-invalid={shown.maxOutput ? 'true' : undefined}
						aria-describedby={shown.maxOutput ? `${uid}-max-output-error` : undefined}
					/>
					{#if shown.maxOutput}
						<Field.Error id={`${uid}-max-output-error`}>{shown.maxOutput}</Field.Error>
					{/if}
				</Field.Field>
			</div>

			<Field.Set data-invalid={shown.modalities ? 'true' : undefined}>
				<Field.Legend variant="label">{t('settings.acceptedInputs')}</Field.Legend>
				<div class="grid grid-cols-2 gap-2 sm:grid-cols-3">
					{#each MODALITY_OPTIONS as modality (modality)}
						<Field.Field orientation="horizontal">
							<Checkbox
								id={`${uid}-modality-${modality}`}
								bind:checked={
									() => form.modalities.includes(modality),
									(on) =>
										(form.modalities = on
											? [...form.modalities, modality]
											: form.modalities.filter((m) => m !== modality))
								}
							/>
							<Field.Label for={`${uid}-modality-${modality}`} class="font-normal">
								{MODALITY_LABELS[modality] ?? modality}
							</Field.Label>
						</Field.Field>
					{/each}
				</div>
				{#if shown.modalities}
					<Field.Error>{shown.modalities}</Field.Error>
				{/if}
			</Field.Set>

			<Field.Set>
				<Field.Legend variant="label">{t('settings.costPerMtok')}</Field.Legend>
				<div class="grid gap-4 sm:grid-cols-3">
					{#each [{ key: 'priceInput', label: t('common.enter') }, { key: 'priceOutput', label: t('common.output') }, { key: 'priceCache', label: t('settings.cache') }] as const as field (field.key)}
						<Field.Field data-invalid={shown[field.key] ? 'true' : undefined}>
							<Field.Label for={`${uid}-${field.key}`}>{field.label}</Field.Label>
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
								<Field.Error id={`${uid}-${field.key}-error`}>{shown[field.key]}</Field.Error>
							{/if}
						</Field.Field>
					{/each}
				</div>
			</Field.Set>

			<Field.Field data-invalid={levelError ? 'true' : undefined}>
				<Field.Label for={`${uid}-level`}>{t('settings.reasoningLevels')}</Field.Label>
				{#if form.profiles.length > 0}
					<ul class="flex flex-wrap items-center gap-1.5" aria-label={t('settings.reasoningLevels')}>
						{#each form.profiles as profile, index (profile.id)}
							<li
								class="inline-flex h-7 max-w-full items-center gap-1 rounded-full border border-border bg-background pr-1 pl-2.5 text-xs text-foreground"
							>
								<span class="truncate">{draftLabel(profile)}</span>
								<button
									type="button"
									class="inline-flex size-5 items-center justify-center rounded-full text-muted-foreground hover:bg-element-hover hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
									aria-label={t('settings.removeLevel', { level: draftLabel(profile) })}
									onclick={() => removeLevel(index)}
								>
									<X class="size-3" aria-hidden="true" />
								</button>
							</li>
						{/each}
					</ul>
				{/if}
				<ButtonGroup.Root class="w-full">
					<Input
						id={`${uid}-level`}
						bind:value={level}
						maxlength={64}
						autocomplete="off"
						spellcheck={false}
						class="font-mono text-sm"
						placeholder={t('settings.level')}
						aria-invalid={levelError ? 'true' : undefined}
						aria-describedby={levelError ? `${uid}-level-error` : undefined}
						oninput={() => (levelError = null)}
						onkeydown={onLevelKey}
					/>
					<Button type="button" variant="outline" disabled={level.trim() === ''} onclick={addLevel}>
						{t('common.add')}
					</Button>
				</ButtonGroup.Root>
				{#if levelError}
					<Field.Error id={`${uid}-level-error`}>{levelError}</Field.Error>
				{/if}
			</Field.Field>

			<label class="flex items-center justify-between gap-3 text-sm">
				{t('settings.asynchronousToolCalls')}
				<Switch bind:checked={form.asyncToolCalls} />
			</label>

			{#if error}
				<p role="alert" class="text-sm text-pretty text-destructive">{error}</p>
			{/if}

			<Dialog.Footer>
				<Button type="button" variant="outline" onclick={onClose}>{t('common.cancel')}</Button>
				<Button type="submit" disabled={saving}>{t('common.save')}</Button>
			</Dialog.Footer>
		</form>
	</Dialog.Content>
</Dialog.Root>
