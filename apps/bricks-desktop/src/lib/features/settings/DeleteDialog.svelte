<script lang="ts">
	// Confirmation of a deletion in the models settings. The write is passed in; a failure stays
	// in the dialog so the user can read it. Mounted only while it is open.
	import * as AlertDialog from '$lib/components/ui/alert-dialog';
	import { Button } from '$lib/components/ui/button';
	import type { WriteOutcome } from './models';

	let {
		title,
		description,
		notice = null,
		onConfirm,
		onClose
	}: {
		title: string;
		description: string;
		/** Set when the dialog was reopened after the file changed elsewhere. */
		notice?: string | null;
		/** Runs the deletion and resolves with its outcome. */
		onConfirm: () => Promise<WriteOutcome>;
		/** The dialog must go: closed by the user, or after the deletion succeeded. */
		onClose: () => void;
	} = $props();

	let busy = $state(false);
	let error = $state<string | null>(null);

	async function confirm() {
		if (busy) return;
		busy = true;
		error = null;
		const outcome = await onConfirm();
		busy = false;
		if (outcome.ok) onClose();
		else error = outcome.message;
	}
</script>

<AlertDialog.Root open onOpenChange={(next) => (next ? undefined : onClose())}>
	<AlertDialog.Content>
		<AlertDialog.Header>
			<AlertDialog.Title>{title}</AlertDialog.Title>
			<AlertDialog.Description>{description}</AlertDialog.Description>
		</AlertDialog.Header>
		{#if notice}
			<p role="alert" class="text-sm text-pretty text-destructive">{notice}</p>
		{/if}
		{#if error}
			<p role="alert" class="text-sm text-pretty text-destructive">{error}</p>
		{/if}
		<AlertDialog.Footer>
			<AlertDialog.Cancel disabled={busy}>Annuler</AlertDialog.Cancel>
			<Button variant="destructive" disabled={busy} onclick={confirm}>Supprimer</Button>
		</AlertDialog.Footer>
	</AlertDialog.Content>
</AlertDialog.Root>
