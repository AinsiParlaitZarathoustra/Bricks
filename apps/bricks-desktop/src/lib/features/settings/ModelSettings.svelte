<script lang="ts">
	// « Modèles » of the settings: the providers of the providers file, each with its models.
	// Reads and writes the file through the IPC; every write carries the revision last read.
	import { onMount } from 'svelte';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import { app } from '$lib/state/app.svelte';
	import {
		errorMessage,
		modelDelete,
		modelUpdate,
		providerDelete,
		providerUpdate,
		providersConfig
	} from '$lib/ipc/client';
	import type { ModelPatch } from '$lib/ipc/bindings/ModelPatch';
	import type { ModelView } from '$lib/ipc/bindings/ModelView';
	import type { ProviderPatch } from '$lib/ipc/bindings/ProviderPatch';
	import type { ProviderView } from '$lib/ipc/bindings/ProviderView';
	import type { ProvidersFile } from '$lib/ipc/bindings/ProvidersFile';
	import RefreshCw from '@lucide/svelte/icons/refresh-cw';
	import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
	import SettingsGroup from './SettingsGroup.svelte';
	import ProviderGroup from './ProviderGroup.svelte';
	import ProviderFormDialog from './ProviderFormDialog.svelte';
	import ModelFormDialog from './ModelFormDialog.svelte';
	import DeleteDialog from './DeleteDialog.svelte';
	import {
		CONFLICT_MESSAGE,
		displayName,
		providerDeletionDetail,
		writeFailure,
		type WriteOutcome
	} from './models';

	/**
	 * The one dialog open at a time, with the provider (and model) it concerns. `notice` is set when
	 * the dialog was reopened after the file changed elsewhere.
	 */
	type Dialog =
		| { kind: 'edit-provider'; provider: ProviderView; notice?: string }
		| { kind: 'edit-model'; provider: ProviderView; model: ModelView; notice?: string }
		| { kind: 'delete-provider'; provider: ProviderView; notice?: string }
		| { kind: 'delete-model'; provider: ProviderView; model: ModelView; notice?: string };

	let file = $state<ProvidersFile | null>(null);
	let loading = $state(false);
	let loadError = $state<string | null>(null);
	let dialog = $state<Dialog | null>(null);
	/** Set when the target of a reopened dialog no longer exists in the file. */
	let pageNotice = $state<string | null>(null);

	const providers = $derived(file?.providers ?? []);

	/** Reads the file. Resolves with whether it could be read. */
	async function load(): Promise<boolean> {
		loading = true;
		try {
			file = await providersConfig();
			loadError = null;
			return true;
		} catch (e) {
			loadError = errorMessage(e);
			return false;
		} finally {
			loading = false;
		}
	}

	onMount(() => {
		void load();
	});

	function openDialog(next: Dialog) {
		pageNotice = null;
		dialog = next;
	}

	function closeDialog() {
		dialog = null;
	}

	/**
	 * Runs one write with the revision last read. The reply replaces the file and the catalogue of
	 * the composer is reloaded. A stale revision goes through `reopenAfterConflict`.
	 */
	async function write(run: (revision: string) => Promise<ProvidersFile>): Promise<WriteOutcome> {
		const current = file;
		if (!current) return { ok: false, message: 'Le fichier des fournisseurs n’est pas encore lu.', conflict: false };
		try {
			file = await run(current.revision);
			void app.reloadModels();
			return { ok: true };
		} catch (e) {
			const outcome = writeFailure(e);
			if (!outcome.ok && outcome.conflict) await reopenAfterConflict();
			return outcome;
		}
	}

	/**
	 * The file changed elsewhere. It is read again and the open dialog starts over from the fresh
	 * values, with a notice: nothing typed before is sent with the stale revision. When the target
	 * is gone, the dialog closes and the page says so. If the file cannot be read, the dialog closes.
	 */
	async function reopenAfterConflict() {
		const open = dialog;
		const reloaded = await load();
		const fresh = file;
		if (!open) return;
		if (!reloaded || !fresh) {
			dialog = null;
			return;
		}
		const provider = fresh.providers.find((p) => p.id === open.provider.id);
		if (!provider) {
			dialog = null;
			pageNotice = `Le fournisseur « ${displayName(open.provider.name, open.provider.id)} » n’existe plus dans le fichier des fournisseurs.`;
			return;
		}
		if (open.kind === 'edit-provider' || open.kind === 'delete-provider') {
			dialog = { kind: open.kind, provider, notice: CONFLICT_MESSAGE };
			return;
		}
		const model = provider.models.find((m) => m.id === open.model.id);
		if (!model) {
			dialog = null;
			pageNotice = `Le modèle « ${displayName(open.model.name, open.model.id)} » n’existe plus dans le fichier des fournisseurs.`;
			return;
		}
		dialog = { kind: open.kind, provider, model, notice: CONFLICT_MESSAGE };
	}

	function saveProvider(provider: ProviderView, patch: ProviderPatch): Promise<WriteOutcome> {
		return write((revision) => providerUpdate(provider.id, patch, revision));
	}

	function saveModel(provider: ProviderView, model: ModelView, patch: ModelPatch): Promise<WriteOutcome> {
		return write((revision) => modelUpdate(provider.id, model.id, patch, revision));
	}

	function removeProvider(provider: ProviderView): Promise<WriteOutcome> {
		return write((revision) => providerDelete(provider.id, revision));
	}

	function removeModel(provider: ProviderView, model: ModelView): Promise<WriteOutcome> {
		return write((revision) => modelDelete(provider.id, model.id, revision));
	}
</script>

<div class="flex flex-col gap-6">
	<section class="flex flex-col gap-3" aria-labelledby="models-providers-title">
		<div class="flex flex-col gap-1">
			<h3 id="models-providers-title" class="text-sm font-semibold text-foreground">Fournisseurs et modèles</h3>
			<p class="text-sm text-pretty text-muted-foreground">
				Cliquez sur le nom d’un modèle pour voir ses détails. Les modifications sont enregistrées dans le fichier
				des fournisseurs.
			</p>
			{#if file}
				<p class="truncate text-xs text-muted-foreground" title={file.path}>
					Fichier : <span class="font-mono">{file.path}</span>
				</p>
			{/if}
		</div>

		{#if pageNotice}
			<p role="alert" class="text-sm text-pretty text-destructive">{pageNotice}</p>
		{/if}

		{#if !file}
			{#if loadError}
				<div role="alert" class="flex flex-col items-start gap-3 rounded-lg border border-destructive/40 bg-destructive/5 p-3 text-sm">
					<p class="flex items-start gap-2 text-pretty">
						<TriangleAlert class="mt-0.5 size-4 shrink-0 text-destructive" aria-hidden="true" />
						Impossible de lire les fournisseurs : {loadError}
					</p>
					<Button variant="outline" size="sm" onclick={load} disabled={loading}>
						<RefreshCw aria-hidden="true" />
						Réessayer
					</Button>
				</div>
			{:else}
				<p role="status" class="text-sm text-muted-foreground">Lecture des fournisseurs…</p>
			{/if}
		{:else}
			{#if loadError}
				<p role="alert" class="text-sm text-pretty text-destructive">Relecture impossible : {loadError}</p>
			{/if}

			{#if file.error}
				<div role="alert" class="flex flex-col items-start gap-3 rounded-lg border border-destructive/40 bg-destructive/5 p-3 text-sm">
					<p class="flex items-start gap-2 text-pretty">
						<TriangleAlert class="mt-0.5 size-4 shrink-0 text-destructive" aria-hidden="true" />
						Le fichier des fournisseurs est invalide : {file.error}
					</p>
					<Button variant="outline" size="sm" onclick={load} disabled={loading}>
						<RefreshCw aria-hidden="true" />
						Réessayer
					</Button>
				</div>
			{/if}

			{#if providers.length === 0}
				{#if !file.error}
					<p class="text-sm text-muted-foreground">Aucun fournisseur configuré.</p>
				{/if}
			{:else}
				<ul class="flex flex-col gap-3" aria-label="Fournisseurs">
					{#each providers as provider (provider.id)}
						<li>
							<ProviderGroup
								{provider}
								onEditProvider={() => openDialog({ kind: 'edit-provider', provider })}
								onDeleteProvider={() => openDialog({ kind: 'delete-provider', provider })}
								onEditModel={(model) => openDialog({ kind: 'edit-model', provider, model })}
								onDeleteModel={(model) => openDialog({ kind: 'delete-model', provider, model })}
							/>
						</li>
					{/each}
				</ul>
			{/if}
		{/if}
	</section>

	<!-- Keyed on the dialog: a reopened dialog (after a conflict) starts again from the fresh values. -->
	{#key dialog}
		{#if dialog?.kind === 'edit-provider'}
			{@const target = dialog.provider}
			<ProviderFormDialog
				provider={target}
				notice={dialog.notice}
				onSave={(patch) => saveProvider(target, patch)}
				onClose={closeDialog}
			/>
		{:else if dialog?.kind === 'edit-model'}
			{@const owner = dialog.provider}
			{@const target = dialog.model}
			<ModelFormDialog
				model={target}
				notice={dialog.notice}
				onSave={(patch) => saveModel(owner, target, patch)}
				onClose={closeDialog}
			/>
		{:else if dialog?.kind === 'delete-provider'}
			{@const target = dialog.provider}
			<DeleteDialog
				title={`Supprimer le fournisseur ${displayName(target.name, target.id)} ?`}
				description={`Le fournisseur disparaît de la configuration de Bricks. ${providerDeletionDetail(target)}`}
				notice={dialog.notice}
				onConfirm={() => removeProvider(target)}
				onClose={closeDialog}
			/>
		{:else if dialog?.kind === 'delete-model'}
			{@const owner = dialog.provider}
			{@const target = dialog.model}
			<DeleteDialog
				title={`Supprimer le modèle ${displayName(target.name, target.id)} ?`}
				description="Le modèle disparaît de la configuration de Bricks."
				notice={dialog.notice}
				onConfirm={() => removeModel(owner, target)}
				onClose={closeDialog}
			/>
		{/if}
	{/key}

	<SettingsGroup title="Choix par défaut" description="Modèle et effort de raisonnement appliqués aux nouvelles conversations.">
		<div class="flex items-center justify-between gap-4 p-3">
			<span class="text-sm text-muted-foreground">Modèle par défaut</span>
			<Badge variant="outline">À venir</Badge>
		</div>
		<div class="flex items-center justify-between gap-4 p-3">
			<span class="text-sm text-muted-foreground">Effort de raisonnement par défaut</span>
			<Badge variant="outline">À venir</Badge>
		</div>
	</SettingsGroup>
</div>
