<script lang="ts">
	// Form of a provider: a new one (`provider` null) or an existing one, from its values when opened.
	// Cancel discards.
	import { t } from '$lib/i18n/i18n.svelte';
	import { untrack } from 'svelte';
	import * as Dialog from '$lib/components/ui/dialog';
	import * as Select from '$lib/components/ui/select';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Label } from '$lib/components/ui/label';
	import KeyRound from '@lucide/svelte/icons/key-round';
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
		type KeyChange,
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
		/**
		 * Writes the patch (every field for a new provider, else only the changed ones; `null` when
		 * none changed), then the key change.
		 */
		onSave: (patch: ProviderPatch | null, id: string, key: KeyChange) => Promise<WriteOutcome>;
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
	/** The saved key is being replaced: the field shows instead of the saved state. */
	let replacing = $state(false);
	/** The saved key goes when the form is saved. */
	let removing = $state(false);

	const hasSavedKey = $derived((original?.has_key ?? false) && !removing);
	const needsKey = $derived(form.auth !== 'none');

	const errors = $derived(validateProviderForm(form, original));
	const idError = $derived(original ? undefined : validateNewId(id, takenIds));
	/** Field messages show once the user tried to save. */
	const shown = $derived(attempted ? errors : {});

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		if (saving) return;
		attempted = true;
		if (hasFormErrors(errors) || idError) return;
		const built = original ? buildProviderPatch(original, form) : buildProviderCreatePatch(form);
		const patch = original && isEmptyProviderPatch(built) ? null : built;
		const typed = form.apiKey.trim();
		const key: KeyChange =
			needsKey && typed !== ''
				? { kind: 'set', value: typed }
				: removing
					? { kind: 'remove' }
					: { kind: 'keep' };
		if (patch === null && key.kind === 'keep') {
			onClose();
			return;
		}
		saving = true;
		error = null;
		const outcome = await onSave(patch, original ? original.id : id.trim(), key);
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
				<div class="flex flex-col gap-2">
					<Label for={`${uid}-id`}>{t('common.identifier')}</Label>
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
				<Label for={`${uid}-name`}>{t('settings.name')}</Label>
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
				<Label for={`${uid}-endpoint`}>{t('settings.apiUrl')}</Label>
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
					<p id={`${uid}-endpoint-error`} role="alert" class="text-xs text-destructive">{shown.endpoint}</p>
				{/if}
			</div>

			<div class="flex flex-col gap-2">
				<Label for={`${uid}-protocol`}>{t('settings.protocol')}</Label>
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
			</div>

			<div class="flex flex-col gap-2">
				<Label for={`${uid}-auth`}>{t('settings.authentication')}</Label>
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
			</div>

			{#if form.auth === 'api_key_header'}
				<div class="flex flex-col gap-2">
					<Label for={`${uid}-header`}>{t('settings.keyHeader')}</Label>
					<Input
						id={`${uid}-header`}
						bind:value={form.authHeader}
						autocomplete="off"
						spellcheck={false}
						class="font-mono text-sm"
					/>
				</div>
			{/if}

			{#if needsKey}
				<div class="flex flex-col gap-2">
					<Label for={`${uid}-key`}>{t('settings.apiKey')}</Label>
					{#if hasSavedKey && !replacing}
						<div class="flex h-9 items-center gap-2 rounded-lg border border-input bg-background pr-1 pl-3">
							<KeyRound class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
							<span id={`${uid}-key`} class="min-w-0 flex-1 truncate text-sm">{t('settings.keySaved')}</span>
							<Button type="button" variant="ghost" size="sm" onclick={() => (replacing = true)}>
								{t('settings.replaceKey')}
							</Button>
							<Button
								type="button"
								variant="ghost"
								size="sm"
								class="text-destructive hover:text-destructive"
								onclick={() => {
									removing = true;
									form.apiKey = '';
								}}
							>
								{t('common.delete')}
							</Button>
						</div>
					{:else}
						<Input
							id={`${uid}-key`}
							type="password"
							bind:value={form.apiKey}
							autocomplete="off"
							spellcheck={false}
							class="font-mono text-sm"
							aria-invalid={shown.apiKey ? 'true' : undefined}
							aria-describedby={shown.apiKey ? `${uid}-key-error` : undefined}
						/>
						{#if shown.apiKey}
							<p id={`${uid}-key-error`} role="alert" class="text-xs text-destructive">{shown.apiKey}</p>
						{/if}
					{/if}
				</div>
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
