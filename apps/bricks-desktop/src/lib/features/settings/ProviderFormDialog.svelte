<script lang="ts">
	// Form of a provider: a new one (`provider` null) or an existing one, from its values when opened.
	// Cancel discards.
	import { t } from '$lib/i18n/i18n.svelte';
	import { untrack } from 'svelte';
	import * as Dialog from '$lib/components/ui/dialog';
	import * as Select from '$lib/components/ui/select';
	import { Button } from '$lib/components/ui/button';
	import { Toggle } from '$lib/components/ui/toggle';
	import * as InputGroup from '$lib/components/ui/input-group';
	import { Input } from '$lib/components/ui/input';
	import * as Field from '$lib/components/ui/field';
	import Eye from '@lucide/svelte/icons/eye';
	import EyeOff from '@lucide/svelte/icons/eye-off';
	import type { ProviderPatch } from '$lib/ipc/bindings/ProviderPatch';
	import type { ProviderView } from '$lib/ipc/bindings/ProviderView';
	import {
		AUTH_OPTIONS,
		PROTOCOL_OPTIONS,
		authLabel,
		buildProviderCreatePatch,
		buildProviderPatch,
		hasFormErrors,
		displayName,
		isEmptyProviderPatch,
		newProviderForm,
		protocolLabel,
		providerForm,
		validateNewId,
		validateProviderForm,
		type ProviderForm,
		type WriteOutcome
	} from './models';

	let {
		provider,
		takenIds = [],
		notice = null,
		onSave,
		onClose
	}: {
		/** `null`: a new provider. */
		provider: ProviderView | null;
		/** Ids already in the file (a new provider needs its own). */
		takenIds?: readonly string[];
		/** Set when the dialog was reopened after the file changed elsewhere. */
		notice?: string | null;
		/** Writes the patch: every field for a new provider, else only the changed ones. */
		onSave: (patch: ProviderPatch, id: string) => Promise<WriteOutcome>;
		/** The dialog must go: closed by the user, or after a successful save. */
		onClose: () => void;
	} = $props();

	const uid = $props.id();
	const original = untrack(() => provider);
	let form = $state<ProviderForm>(original ? providerForm(original) : newProviderForm());
	let id = $state('');
	let attempted = $state(false);
	let saving = $state(false);
	let error = $state<string | null>(null);
	/** The key shows in clear while this is on. */
	let revealed = $state(false);

	const needsKey = $derived(form.auth !== 'none');

	const errors = $derived(validateProviderForm(form));
	const idError = $derived(original ? undefined : validateNewId(id, takenIds));
	/** Field messages show once the user tried to save. */
	const shown = $derived(attempted ? errors : {});

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		if (saving) return;
		attempted = true;
		if (hasFormErrors(errors) || idError) return;
		const patch = original ? buildProviderPatch(original, form) : buildProviderCreatePatch(form);
		if (original && isEmptyProviderPatch(patch)) {
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
	<Dialog.Content class="max-h-[min(44rem,calc(100dvh-2rem))] overflow-y-auto sm:max-w-lg">
		<form onsubmit={submit} class="flex flex-col gap-5">
			<Dialog.Header>
				<Dialog.Title>
					{original ? t('settings.editProvider', { id: displayName(original.name, original.id) }) : t('settings.addAProvider')}
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

			<Field.Field data-invalid={shown.endpoint ? 'true' : undefined}>
				<Field.Label for={`${uid}-endpoint`}>{t('settings.apiUrl')}</Field.Label>
				<Input
					id={`${uid}-endpoint`}
					bind:value={form.endpoint}
					autocomplete="off"
					spellcheck={false}
					class="font-mono text-sm"
					aria-invalid={shown.endpoint ? 'true' : undefined}
					aria-describedby={shown.endpoint ? `${uid}-endpoint-error` : undefined}
				/>
				{#if shown.endpoint}
					<Field.Error id={`${uid}-endpoint-error`}>{shown.endpoint}</Field.Error>
				{/if}
			</Field.Field>

			<Field.Field>
				<Field.Label for={`${uid}-protocol`}>{t('settings.protocol')}</Field.Label>
				<Select.Root
					type="single"
					value={form.protocol}
					onValueChange={(next) => {
						if (next !== undefined) form.protocol = next;
					}}
				>
					<Select.Trigger id={`${uid}-protocol`} class="w-full">
						<span class="truncate">{protocolLabel(form.protocol)}</span>
					</Select.Trigger>
					<Select.Content>
						{#each PROTOCOL_OPTIONS as option (option.value)}
							<Select.Item value={option.value} label={option.label}>{option.label}</Select.Item>
						{/each}
					</Select.Content>
				</Select.Root>
			</Field.Field>

			<Field.Field>
				<Field.Label for={`${uid}-auth`}>{t('settings.authentication')}</Field.Label>
				<Select.Root
					type="single"
					value={form.auth}
					onValueChange={(next) => {
						if (next !== undefined) form.auth = next;
					}}
				>
					<Select.Trigger id={`${uid}-auth`} class="w-full">
						<span class="truncate">{authLabel(form.auth)}</span>
					</Select.Trigger>
					<Select.Content>
						{#each AUTH_OPTIONS as option (option.value)}
							<Select.Item value={option.value} label={option.label}>{option.label}</Select.Item>
						{/each}
					</Select.Content>
				</Select.Root>
			</Field.Field>

			{#if form.auth === 'api_key_header'}
				<Field.Field>
					<Field.Label for={`${uid}-header`}>{t('settings.keyHeader')}</Field.Label>
					<Input
						id={`${uid}-header`}
						bind:value={form.authHeader}
						autocomplete="off"
						spellcheck={false}
						class="font-mono text-sm"
					/>
				</Field.Field>
			{/if}

			{#if needsKey}
				<Field.Field data-invalid={shown.apiKey ? 'true' : undefined}>
					<Field.Label for={`${uid}-key`}>{t('settings.apiKey')}</Field.Label>
					<InputGroup.Root>
						<InputGroup.Input
							id={`${uid}-key`}
							type={revealed ? 'text' : 'password'}
							bind:value={form.apiKey}
							autocomplete="off"
							spellcheck={false}
							class="font-mono text-sm"
							aria-invalid={shown.apiKey ? 'true' : undefined}
							aria-describedby={shown.apiKey ? `${uid}-key-error` : undefined}
						/>
						<InputGroup.Addon align="inline-end">
							<Toggle
								size="icon-xs"
								aria-label={revealed ? t('settings.hideKey') : t('settings.showKey')}
								bind:pressed={revealed}
							>
								{#if revealed}
									<EyeOff aria-hidden="true" />
								{:else}
									<Eye aria-hidden="true" />
								{/if}
							</Toggle>
						</InputGroup.Addon>
					</InputGroup.Root>
					{#if shown.apiKey}
						<Field.Error id={`${uid}-key-error`}>{shown.apiKey}</Field.Error>
					{/if}
				</Field.Field>
			{/if}

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
