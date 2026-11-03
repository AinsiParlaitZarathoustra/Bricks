<script lang="ts">
	// Effective configuration of the session (`/config`): model, permissions
	// (edited here), memory, sub-agents, configuration files read, other settings. Shows no key
	// and no authentication header: the engine never sends them.
	import { t } from '$lib/i18n/i18n.svelte';
	import { untrack } from 'svelte';
	import PolicyEditor from '$lib/features/permissions/PolicyEditor.svelte';
	import {
		errorMessage,
		sessionConfig,
		sessionTools,
		type SessionConfigInfo
	} from '$lib/ipc/client';
	import type { SessionPolicy } from '$lib/ipc/engine';
	import EmptyState from '$lib/layout/EmptyState.svelte';
	import SectionCard from '$lib/layout/SectionCard.svelte';
	import { app } from '$lib/state/app.svelte';
	import type { InspectorState } from '$lib/state/ui.svelte';

	// Opened without options: the view shows the conversation on screen.
	let { args: _args }: { args: InspectorState['args'] } = $props();

	const conv = $derived(app.current);
	const sessionId = $derived(conv?.sessionId ?? null);

	let config = $state<SessionConfigInfo | null>(null);
	let loadError = $state<string | null>(null);
	let toolNames = $state<string[]>([]);
	let saving = $state(false);
	let refusal = $state<string | null>(null);

	/** The policy in force: the latest the conversation saw, else the one read. */
	const policy = $derived(conv?.permissions?.policy ?? config?.permission_policy ?? null);

	/** Only the newest read may write the view (a slower earlier one is dropped). */
	let loadToken = 0;

	async function load(id: string) {
		const token = ++loadToken;
		try {
			const [next, tools] = await Promise.all([sessionConfig(id), sessionTools(id)]);
			if (token !== loadToken) return;
			config = next;
			toolNames = tools.map((t) => t.name);
			loadError = null;
		} catch (e) {
			if (token !== loadToken) return;
			loadError = errorMessage(e);
		}
	}

	// Read on open, and again when a run ends or the policy changes.
	$effect(() => {
		const id = sessionId;
		void conv?.running;
		void conv?.permissions?.revision;
		if (!id) {
			loadToken++;
			config = null;
			loadError = null;
			return;
		}
		untrack(() => void load(id));
	});

	/** Sends the change; the editor follows once the engine reports it. */
	async function change(next: SessionPolicy) {
		if (!sessionId) return;
		saving = true;
		refusal = await app.setPermissions(sessionId, next);
		saving = false;
	}
</script>

{#snippet field(label: string, value: string)}
	<dt class="text-muted-foreground">{label}</dt>
	<dd class="min-w-0 break-words">{value}</dd>
{/snippet}

{#if !conv || !sessionId}
	<EmptyState
		title={t('common.noOpenConversation2')}
	/>
{:else if loadError}
	<p role="alert" class="text-sm text-destructive">{t('sessionInfo.couldNotReadTheConfiguration', { loadError })}</p>
{:else if !config}
	<p class="text-sm text-muted-foreground" aria-live="polite">{t('sessionInfo.loadingTheConfiguration')}</p>
{:else}
	<div class="flex flex-col gap-3">
		<SectionCard title={t('common.model')}>
			<dl class="grid grid-cols-[auto_minmax(0,1fr)] items-baseline gap-x-6 gap-y-2 px-3 pb-3 text-sm">
				{@render field(t('sessionInfo.provider'), config.provider || t('common.unknownValue'))}
				{@render field(t('common.model'), config.model || t('common.unknownValue'))}
				{@render field(t('sessionInfo.effort'), config.effective_reasoning ?? t('common.noValue'))}
				{@render field(t('sessionInfo.workingFolder'), config.working_dir)}
			</dl>
		</SectionCard>

		<SectionCard title={t('common.permissions')}>
			<div class="flex flex-col gap-4 px-3 pb-3 text-sm">
				<dl class="grid grid-cols-[auto_minmax(0,1fr)] items-baseline gap-x-6 gap-y-2">
					{@render field(t('sessionInfo.approvals'), config.interactive ? t('sessionInfo.asked') : t('sessionInfo.refused'))}
				</dl>

				{#if policy}
					<PolicyEditor {policy} tools={toolNames} disabled={saving} onchange={change} />
				{/if}
				{#if refusal}
					<p role="alert" class="text-xs break-words text-destructive">{refusal}</p>
				{/if}
			</div>
		</SectionCard>

		<SectionCard title={t('common.memory')}>
			<dl class="grid grid-cols-[auto_minmax(0,1fr)] items-baseline gap-x-6 gap-y-2 px-3 pb-3 text-sm">
				{@render field(t('sessionInfo.workspace'), config.memory_space ?? t('sessionInfo.disabled'))}
			</dl>
		</SectionCard>

		<SectionCard title={t('common.agents')}>
			<dl class="grid grid-cols-[auto_minmax(0,1fr)] items-baseline gap-x-6 gap-y-2 px-3 pb-3 text-sm">
				{@render field(t('common.subAgents'), config.agents_enabled ? t('sessionInfo.enabled') : t('sessionInfo.disabled2'))}
			</dl>
		</SectionCard>

		<SectionCard title={t('sessionInfo.sources')} count={config.sources.length}>
			<div class="flex flex-col gap-2 px-3 pb-3 text-sm">
				{#if config.sources.length > 0}
					<ul class="flex flex-col gap-1">
						{#each config.sources as source, i (i)}
							<li class="font-mono text-xs break-all">{source}</li>
						{/each}
					</ul>
				{:else}
					<p class="text-muted-foreground">{t('sessionInfo.noFileRead')}</p>
				{/if}
			</div>
		</SectionCard>

		<SectionCard title={t('sessionInfo.otherSettings')} count={config.settings.length}>
			<dl class="grid grid-cols-[auto_minmax(0,1fr)] items-baseline gap-x-6 gap-y-2 px-3 pb-3 text-sm">
				{#each config.settings as entry, i (i)}
					<dt class="text-muted-foreground">{entry.label}</dt>
					<dd class="min-w-0 break-words">{entry.value}</dd>
				{/each}
			</dl>
		</SectionCard>
	</div>
{/if}
