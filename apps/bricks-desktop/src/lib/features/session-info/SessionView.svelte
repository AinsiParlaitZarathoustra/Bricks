<script lang="ts">
	// Session inspector (`/session`): identifier, title, folder, model and effort
	// (requested and applied), activity, memory space, approvals.
	import { t } from '$lib/i18n/i18n.svelte';
	import { onDestroy, untrack } from 'svelte';
	import CheckIcon from '@lucide/svelte/icons/check';
	import CopyIcon from '@lucide/svelte/icons/copy';
	import { Button } from '$lib/components/ui/button';
	import { errorMessage, sessionSnapshot } from '$lib/ipc/client';
	import type { Snapshot } from '$lib/ipc/engine';
	import EmptyState from '$lib/layout/EmptyState.svelte';
	import { app } from '$lib/state/app.svelte';
	import type { InspectorState } from '$lib/state/ui.svelte';
	import { displayTitle } from '$lib/features/navigation/format';
	import { activityLabel } from './format';

	// Opened without options: the view shows the conversation on screen.
	let { args: _args }: { args: InspectorState['args'] } = $props();

	const conv = $derived(app.current);
	const sessionId = $derived(conv?.sessionId ?? null);

	let snapshot = $state<Snapshot | null>(null);
	let loadError = $state<string | null>(null);
	let copied = $state(false);
	let copyError = $state<string | null>(null);

	/** Only the newest read may write the view (a slower earlier one is dropped). */
	let loadToken = 0;

	async function load(id: string) {
		const token = ++loadToken;
		try {
			const next = await sessionSnapshot(id);
			if (token !== loadToken) return;
			snapshot = next;
			loadError = null;
		} catch (e) {
			if (token !== loadToken) return;
			loadError = errorMessage(e);
		}
	}

	// Read on open, then again when a run or a command starts or ends, since the
	// activity and the approvals come from the engine's snapshot.
	$effect(() => {
		const id = sessionId;
		void conv?.running;
		void conv?.busy;
		if (!id) {
			loadToken++;
			snapshot = null;
			loadError = null;
			return;
		}
		untrack(() => void load(id));
	});

	let copyTimer: ReturnType<typeof setTimeout> | undefined;
	onDestroy(() => clearTimeout(copyTimer));

	async function copyId() {
		if (!sessionId) return;
		copyError = null;
		try {
			await navigator.clipboard.writeText(sessionId);
			copied = true;
			clearTimeout(copyTimer);
			copyTimer = setTimeout(() => (copied = false), 1500);
		} catch {
			copyError = t('sessionInfo.couldNotCopySelectTheIdentifier');
		}
	}

	const activity = $derived(snapshot ? activityLabel(snapshot.activity) : t('common.loading'));
</script>

{#if !conv || !sessionId}
	<EmptyState
		title={t('common.noOpenConversation2')}
	/>
{:else}
	<div class="flex flex-col gap-4">
		<dl class="grid grid-cols-[auto_minmax(0,1fr)] items-baseline gap-x-6 gap-y-3 text-sm">
			<dt class="text-muted-foreground">{t('common.identifier')}</dt>
			<dd class="flex min-w-0 items-center gap-2">
				<code class="min-w-0 break-all font-mono text-xs select-all">{sessionId}</code>
				<Button
					variant="ghost"
					size="icon-sm"
					onclick={copyId}
					aria-label={copied ? t('sessionInfo.identifierCopied') : t('sessionInfo.copyTheIdentifier')}
				>
					{#if copied}
						<CheckIcon aria-hidden="true" />
					{:else}
						<CopyIcon aria-hidden="true" />
					{/if}
				</Button>
			</dd>

			<dt class="text-muted-foreground">{t('sessionInfo.title')}</dt>
			<dd class="min-w-0 break-words">{displayTitle(conv.title)}</dd>

			<dt class="text-muted-foreground">{t('common.folder')}</dt>
			<dd class="min-w-0 font-mono text-xs break-all">{conv.workingDir || t('common.unknownValue')}</dd>

			<dt class="text-muted-foreground">{t('common.model')}</dt>
			<dd class="min-w-0 font-mono text-xs break-all">{conv.model || t('common.unknownValue')}</dd>

			<dt class="text-muted-foreground">{t('sessionInfo.effort')}</dt>
			<dd>{conv.effectiveReasoning ?? t('common.noValue')}</dd>

			<dt class="text-muted-foreground">{t('sessionInfo.activity')}</dt>
			<dd>{activity}</dd>

			<dt class="text-muted-foreground">{t('sessionInfo.memorySpace')}</dt>
			<dd class="min-w-0 break-words">
				{#if snapshot}
					{snapshot.memory_space ?? t('sessionInfo.noneLongTermMemoryDisabled')}
				{:else}
					{t('common.loading')}
				{/if}
			</dd>

			<dt class="text-muted-foreground">{t('sessionInfo.approvals')}</dt>
			<dd>
				{#if snapshot}
					{snapshot.interactive
						? t('sessionInfo.askedOfAPerson')
						: t('sessionInfo.nobodyToAnswerActionsNeedingApproval')}
				{:else}
					{t('common.loading')}
				{/if}
			</dd>
		</dl>

		{#if copyError}
			<p role="alert" class="text-sm text-destructive">{copyError}</p>
		{/if}
		{#if loadError}
			<p role="alert" class="text-sm text-destructive">
				{t('sessionInfo.couldNotReadTheSession', { loadError })}
			</p>
		{/if}
	</div>
{/if}
