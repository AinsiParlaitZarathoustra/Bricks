<script lang="ts">
	// Edit form of a provider. Starts from the provider as it was when opened; Cancel discards.
	import { untrack } from 'svelte';
	import * as Dialog from '$lib/components/ui/dialog';
	import * as Select from '$lib/components/ui/select';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Label } from '$lib/components/ui/label';
	import type { ProviderPatch } from '$lib/ipc/bindings/ProviderPatch';
	import type { ProviderView } from '$lib/ipc/bindings/ProviderView';
	import {
		AUTH_OPTIONS,
		PROTOCOL_OPTIONS,
		authLabel,
		buildProviderPatch,
		hasFormErrors,
		displayName,
		isEmptyProviderPatch,
		protocolLabel,
		providerForm,
		validateProviderForm,
		type ProviderForm,
		type WriteOutcome
	} from './models';

	let {
		provider,
		notice = null,
		onSave,
		onClose
	}: {
		provider: ProviderView;
		/** Set when the dialog was reopened after the file changed elsewhere. */
		notice?: string | null;
		/** Writes the patch (only the changed fields); resolves with the outcome. */
		onSave: (patch: ProviderPatch) => Promise<WriteOutcome>;
		/** The dialog must go: closed by the user, or after a successful save. */
		onClose: () => void;
	} = $props();

	const uid = $props.id();
	const original = untrack(() => provider);
	let form = $state<ProviderForm>(providerForm(original));
	let attempted = $state(false);
	let saving = $state(false);
	let error = $state<string | null>(null);

	const errors = $derived(validateProviderForm(form, original));
	/** Field messages show once the user tried to save. */
	const shown = $derived(attempted ? errors : {});

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		if (saving) return;
		attempted = true;
		if (hasFormErrors(errors)) return;
		const patch = buildProviderPatch(original, form);
		if (isEmptyProviderPatch(patch)) {
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
	<Dialog.Content class="max-h-[min(44rem,calc(100dvh-2rem))] overflow-y-auto sm:max-w-lg">
		<form onsubmit={submit} class="flex flex-col gap-5">
			<Dialog.Header>
				<Dialog.Title>Modifier le fournisseur {displayName(original.name, original.id)}</Dialog.Title>
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
				<Label for={`${uid}-endpoint`}>Adresse de l’API</Label>
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
				<Label for={`${uid}-protocol`}>Protocole</Label>
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
				<Label for={`${uid}-auth`}>Authentification</Label>
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
					<Label for={`${uid}-header`}>En-tête de la clé</Label>
					<Input
						id={`${uid}-header`}
						bind:value={form.authHeader}
						autocomplete="off"
						spellcheck={false}
						class="font-mono text-sm"
					/>
				</div>
			{/if}

			<div class="flex flex-col gap-2">
				<Label for={`${uid}-env`}>Variable d’environnement de la clé</Label>
				<Input
					id={`${uid}-env`}
					bind:value={form.apiKeyEnv}
					disabled={original.has_inline_key}
					autocomplete="off"
					spellcheck={false}
					class="font-mono text-sm"
					aria-invalid={shown.apiKeyEnv ? 'true' : undefined}
					aria-describedby={`${uid}-env-help`}
				/>
				{#if shown.apiKeyEnv}
					<p id={`${uid}-env-help`} role="alert" class="text-xs text-pretty text-destructive">{shown.apiKeyEnv}</p>
				{:else}
					<p id={`${uid}-env-help`} class="text-xs text-pretty text-muted-foreground">
						{original.has_inline_key
							? 'Une clé est écrite dans le fichier : la variable ne peut pas être définie ici.'
							: 'Nom de la variable d’environnement qui contient la clé.'}
					</p>
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
