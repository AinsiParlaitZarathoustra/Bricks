<script lang="ts">
	// Tools of the session (`/tools`): name, description, permission level and
	// the decision of the session's permission policy.
	import { t } from '$lib/i18n/i18n.svelte';
	import { untrack } from 'svelte';
	import { Badge } from '$lib/components/ui/badge';
	import { Input } from '$lib/components/ui/input';
	import { errorMessage, sessionTools, type SessionToolInfo } from '$lib/ipc/client';
	import EmptyState from '$lib/layout/EmptyState.svelte';
	import { app } from '$lib/state/app.svelte';
	import type { InspectorState } from '$lib/state/ui.svelte';
	import {
		decisionLabel,
		decisionVariant,
		levelLabel,
		matchesTool
	} from './format';

	// Opened without options: the view shows the conversation on screen.
	let { args: _args }: { args: InspectorState['args'] } = $props();

	const conv = $derived(app.current);
	const sessionId = $derived(conv?.sessionId ?? null);

	let tools = $state<SessionToolInfo[] | null>(null);
	let loadError = $state<string | null>(null);
	let filter = $state('');

	/** Only the newest read may write the view (a slower earlier one is dropped). */
	let loadToken = 0;

	async function load(id: string) {
		const token = ++loadToken;
		try {
			const next = await sessionTools(id);
			if (token !== loadToken) return;
			tools = next;
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
			tools = null;
			loadError = null;
			return;
		}
		untrack(() => void load(id));
	});

	const visible = $derived(tools ? tools.filter((t) => matchesTool(t, filter)) : []);
</script>

{#if !conv || !sessionId}
	<EmptyState
		title={t('common.noOpenConversation2')}
	/>
{:else if loadError}
	<p role="alert" class="text-sm text-destructive">{t('sessionInfo.couldNotReadTheTools', { loadError })}</p>
{:else if tools === null}
	<p class="text-sm text-muted-foreground" aria-live="polite">{t('sessionInfo.loadingTools')}</p>
{:else if tools.length === 0}
	<EmptyState title={t('sessionInfo.noToolAvailable')} />
{:else}
	<div class="flex flex-col gap-3">
		<Input
			type="search"
			bind:value={filter}
			placeholder={t('sessionInfo.filterByNameOrDescription')}
			aria-label={t('sessionInfo.filterTools')}
			autocomplete="off"
		/>

		{#if visible.length === 0}
			<p class="text-sm text-muted-foreground">{t('sessionInfo.noToolMatches', { filter: filter.trim() })}</p>
		{:else}
			<p class="text-xs text-muted-foreground" aria-live="polite">
				{visible.length === tools.length
					? t('sessionInfo.toolCount', { count: tools.length })
					: t('sessionInfo.ofTools', { visible: visible.length, count: tools.length })}
			</p>
			<div class="overflow-x-auto rounded-lg border border-border">
				<table class="w-full min-w-[32rem] text-left text-sm">
					<caption class="sr-only">{t('sessionInfo.sessionToolsWithTheirLevelAnd')}</caption>
					<thead class="bg-muted text-xs text-muted-foreground">
						<tr>
							<th scope="col" class="px-3 py-2 font-medium">{t('sessionInfo.tool')}</th>
							<th scope="col" class="px-3 py-2 font-medium">{t('sessionInfo.level')}</th>
							<th scope="col" class="px-3 py-2 font-medium">{t('sessionInfo.decision')}</th>
						</tr>
					</thead>
					<tbody class="divide-y divide-border">
						{#each visible as tool, i (i)}
							<tr class="align-top">
								<th scope="row" class="px-3 py-2 font-mono text-xs font-medium break-all">
									{tool.name}
									{#if tool.description}
										<p class="mt-0.5 font-sans text-xs font-normal text-muted-foreground">
											{tool.description}
										</p>
									{/if}
								</th>
								<td class="px-3 py-2 whitespace-nowrap">{levelLabel(tool.level)}</td>
								<td class="px-3 py-2">
									<Badge variant={decisionVariant(tool.decision)} title={tool.reason}>
										{decisionLabel(tool.decision)}
									</Badge>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}
	</div>
{/if}
