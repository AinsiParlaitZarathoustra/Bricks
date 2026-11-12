<script lang="ts">
	// Clearing the active context (`/clear`). Says what it does and does not do,
	// asks for the confirmation, then shows the engine's answer.
	import { t } from '$lib/i18n/i18n.svelte';
	import type { InspectorState } from '$lib/state/ui.svelte';
	import { app } from '$lib/state/app.svelte';
	import { ui } from '$lib/state/ui.svelte';
	import { Button } from '$lib/components/ui/button';
	import * as Alert from '$lib/components/ui/alert';
	import { Spinner } from '$lib/components/ui/spinner';

	let { args }: { args: InspectorState['args'] } = $props();

	const conv = $derived(app.current);
	const busy = $derived(conv ? conv.running || conv.busy !== null : false);

	/**
	 * The request sent: the `lastClear` and the last rejection seen at that time.
	 * The answer is a newer `lastClear` (set by `context_cleared`), or a
	 * `command_rejected` for `clear_context` newer than the rejection seen.
	 */
	let sent = $state<{ clearedAt: number | null; rejectionAt: number | null } | null>(null);
	let actionError = $state<string | null>(null);

	const cleared = $derived.by(() => {
		const c = conv?.lastClear;
		if (!sent || !c) return null;
		return sent.clearedAt === null || c.at > sent.clearedAt ? c : null;
	});
	const refusal = $derived.by(() => {
		const r = conv?.lastRejection;
		if (!sent || !r || r.command !== 'clear_context') return null;
		return sent.rejectionAt === null || r.at > sent.rejectionAt ? r.reason : null;
	});
	const pending = $derived(sent !== null && cleared === null && refusal === null);

	async function clear() {
		const current = app.current;
		if (!current) return;
		actionError = null;
		sent = {
			clearedAt: current.lastClear?.at ?? null,
			rejectionAt: current.lastRejection?.at ?? null
		};
		const refused = await app.command(current.sessionId, { type: 'clear_context' });
		if (refused !== null) {
			sent = null;
			actionError = refused;
		}
	}
</script>

<div class="flex min-w-0 flex-col gap-4">
	{#if !conv}
		<p class="text-sm text-muted-foreground">{t('common.noOpenConversation')}</p>
	{:else}
		{#if conv.running}
			<Alert.Root>
				<Alert.Description>{t('inspect.notPossibleDuringARun')}</Alert.Description>
			</Alert.Root>
		{:else if conv.busy !== null}
			<Alert.Root>
				<Alert.Description>{t('inspect.anOperationIsInProgress')}</Alert.Description>
			</Alert.Root>
		{/if}

		<div class="flex flex-wrap items-center gap-2" role="status" aria-live="polite">
			{#if cleared !== null}
				<p class="text-sm text-pretty">
					{t('inspect.contextCleared', { count: cleared.messagesRemoved })}
				</p>
				<Button variant="outline" onclick={() => ui.close()}>{t('common.close')}</Button>
			{:else if pending}
				<p class="flex items-center gap-2 text-sm text-muted-foreground">
					<Spinner aria-hidden="true" />
					{t('inspect.clearRequestedWaitingForTheEngines')}
				</p>
			{:else}
				<Button onclick={clear} disabled={busy}>{t('common.clearContext')}</Button>
				<Button variant="ghost" onclick={() => ui.close()}>{t('common.cancel')}</Button>
			{/if}
		</div>

		{#if refusal !== null}
			<p role="alert" class="text-sm text-destructive">{t('inspect.clearRefused', { refusal })}</p>
		{/if}
		{#if actionError}
			<p role="alert" class="text-sm text-destructive">{actionError}</p>
		{/if}
	{/if}
</div>
