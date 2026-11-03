<script lang="ts">
	import { t } from '$lib/i18n/i18n.svelte';
	import { onMount } from 'svelte';
	import * as AlertDialog from '$lib/components/ui/alert-dialog';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import {
		changeStatusLabel,
		changesetLabel,
		changesetVariant,
		formatDuration
	} from '$lib/features/agents/agentView';
	import DiffView from '$lib/features/changes/DiffView.svelte';
	import DiffStat from '$lib/layout/DiffStat.svelte';
	import type { ChangeSet } from '$lib/ipc/engine';
	import { app } from '$lib/state/app.svelte';
	import type { InspectorState } from '$lib/state/ui.svelte';
	import { argText, splitShortened } from './labels';
	import { EngineRequest, controlSlot, type ControlAnswer } from './request.svelte';

	let { args }: { args: InspectorState['args'] } = $props();

	type Write = 'apply_changes' | 'discard_changes';

	const conv = $derived(app.current);
	const argId = $derived(argText(args.changesetId));
	/** What the caller asked: `inspect` (default), `apply` or `discard`. */
	const requested = $derived(argText(args.action) ?? 'inspect');
	/** A change picked in the list; `''` shows the list again. */
	let picked = $state<string | null>(null);
	const selectedId = $derived(picked === null ? argId : picked || null);
	const change = $derived(conv && selectedId ? (conv.changesets.get(selectedId) ?? null) : null);
	const changes = $derived(conv ? [...conv.changesets.values()] : []);

	/** Inspection of a change this conversation does not hold. */
	const inspect = new EngineRequest();
	const inspectAnswer = $derived(inspect.answered<ControlAnswer>());
	/** Apply or discard. */
	const act = new EngineRequest();
	const actAnswer = $derived(act.answered<ControlAnswer>());

	let confirmOpen = $state(false);
	let confirmWrite = $state<Write>('apply_changes');

	/** Apply and discard both need a ready change or one in conflict. */
	function actionable(c: ChangeSet): boolean {
		return c.state === 'ready' || c.state === 'conflict';
	}

	function ask(write: Write) {
		confirmWrite = write;
		confirmOpen = true;
	}

	function writeConfirmed() {
		const id = selectedId;
		if (id) {
			void act.send(
				{ type: 'agent_control', action: confirmWrite, changeset_id: id },
				controlSlot,
				confirmWrite
			);
		}
	}

	onMount(() => {
		const id = selectedId;
		if (!id) return;
		if (change) {
			if (requested === 'apply' && actionable(change)) ask('apply_changes');
			else if (requested === 'discard' && actionable(change)) ask('discard_changes');
			return;
		}
		// Not in this conversation's state: the engine is asked for its patch.
		if (requested === 'inspect') {
			void inspect.send(
				{ type: 'agent_control', action: 'inspect_changes', changeset_id: id },
				controlSlot,
				'inspect_changes'
			);
		}
	});
</script>

<div class="flex flex-col gap-4 text-sm">
	{#if !conv}
		<p class="text-muted-foreground">{t('common.noOpenConversation')}</p>
	{:else if change}
		<div class="flex flex-wrap items-center gap-2">
			<Badge variant={changesetVariant(change.state)}>{changesetLabel(change.state)}</Badge>
			<span class="text-xs text-muted-foreground">{t('common.fileCount', { count: change.files.length })}</span>
			<span class="min-w-0 font-mono text-xs break-all text-muted-foreground">{change.branch}</span>
			{#if changes.length > 1}
				<Button variant="ghost" size="xs" class="ml-auto" onclick={() => (picked = '')}>
					{t('catalog.changeList')}
				</Button>
			{/if}
		</div>

		<p class="text-pretty">{change.task}</p>
		{#if change.note}
			<p class="text-xs text-pretty text-muted-foreground">{change.note}</p>
		{/if}

		<dl class="grid grid-cols-[6.5rem_minmax(0,1fr)] gap-x-3 gap-y-1.5 text-xs">
			<dt class="text-muted-foreground">{t('common.agent2')}</dt>
			<dd class="font-mono break-all">{change.agent_id}</dd>
			<dt class="text-muted-foreground">{t('catalog.isolatedWorkspace')}</dt>
			<dd class="font-mono break-all">{change.workspace}</dd>
		</dl>

		{#if change.files.length > 0}
			<ul class="flex flex-col gap-1 text-xs">
				{#each change.files as file (file.path)}
					<li class="flex min-w-0 items-center gap-2">
						<span class="w-16 shrink-0 text-muted-foreground">{changeStatusLabel(file.status)}</span>
						<span class="min-w-0 flex-1 font-mono break-all">
							{file.path}
							{#if file.from}<span class="text-muted-foreground">← {file.from}</span>{/if}
						</span>
						{#if file.binary}
							<span class="shrink-0 text-muted-foreground">{t('common.binary')}</span>
						{:else}
							<DiffStat additions={file.added ?? 0} deletions={file.removed ?? 0} class="shrink-0" />
						{/if}
					</li>
				{/each}
			</ul>
		{/if}

		{#if change.validations.length > 0}
			<section aria-label={t('catalog.checks')} class="flex flex-col gap-1">
				<h3 class="text-xs font-medium text-muted-foreground">{t('catalog.checks')}</h3>
				<ul class="flex flex-col gap-1 text-xs">
					{#each change.validations as check, i (i)}
						<li class="flex min-w-0 items-center gap-2">
							<span class="min-w-0 flex-1 truncate font-mono">{check.command}</span>
							{#if check.is_error}
								<span class="shrink-0 text-destructive">{t('catalog.error')}</span>
							{/if}
							<span class="shrink-0 tabular-nums text-muted-foreground">{formatDuration(check.duration_ms)}</span>
						</li>
					{/each}
				</ul>
			</section>
		{/if}

		{#if actionable(change)}
			<div class="flex flex-wrap gap-2">
				<Button size="sm" disabled={act.pending} onclick={() => ask('apply_changes')}>{t('catalog.apply')}</Button>
				<Button variant="outline" size="sm" disabled={act.pending} onclick={() => ask('discard_changes')}>
					{t('catalog.discard')}
				</Button>
			</div>
		{/if}

		{#if act.pending}
			<p role="status" class="text-xs text-muted-foreground">{t('catalog.waitingForTheEnginesAnswer')}</p>
		{:else if act.refusal}
			<p role="alert" class="rounded-md bg-destructive/10 px-3 py-2 text-xs text-pretty text-destructive">
				{t('catalog.commandRefused', { refusal: act.refusal })}
			</p>
		{:else if actAnswer}
			{#if actAnswer.ok}
				<p role="status" class="text-xs text-pretty text-muted-foreground">
					{t('catalog.engineAnswer', { text: actAnswer.text })}
				</p>
			{:else}
				<p role="alert" class="rounded-md bg-destructive/10 px-3 py-2 text-xs text-pretty text-destructive">
					{actAnswer.action === 'apply_changes' ? t('common.couldNotApply') : t('common.couldNotDiscard')} :
					{actAnswer.text}
				</p>
			{/if}
		{/if}

		{#if change.state === 'conflict'}
			<p role="alert" class="rounded-md bg-warning-soft px-3 py-2 text-xs text-pretty">
				{t('catalog.conflictThisChangeNoLongerApplies')}
			</p>
		{/if}

		<section aria-label={t('common.diff')} class="flex flex-col gap-1">
			<h3 class="text-xs font-medium text-muted-foreground">{t('common.changes')}</h3>
			<DiffView patch={change.patch} />
		</section>

		<AlertDialog.Root bind:open={confirmOpen}>
			<AlertDialog.Content>
				<AlertDialog.Header>
					{#if confirmWrite === 'apply_changes'}
						<AlertDialog.Title>{t('catalog.applyThisChange')}</AlertDialog.Title>
						<AlertDialog.Description>
							{t('catalog.filesWillBeWritten', { count: change.files.length })}
						</AlertDialog.Description>
					{:else}
						<AlertDialog.Title>{t('catalog.discardThisChange')}</AlertDialog.Title>
						<AlertDialog.Description>
							{t('catalog.theIsolatedChangeWillBeDeleted')}
						</AlertDialog.Description>
					{/if}
				</AlertDialog.Header>
				<AlertDialog.Footer>
					<AlertDialog.Cancel>{t('common.cancel')}</AlertDialog.Cancel>
					<AlertDialog.Action
						variant={confirmWrite === 'discard_changes' ? 'destructive' : 'default'}
						onclick={writeConfirmed}
					>
						{confirmWrite === 'apply_changes' ? t('catalog.apply') : t('catalog.discard')}
					</AlertDialog.Action>
				</AlertDialog.Footer>
			</AlertDialog.Content>
		</AlertDialog.Root>
	{:else if selectedId}
		{#if requested !== 'inspect'}
			<p role="alert" class="rounded-md bg-destructive/10 px-3 py-2 text-xs text-pretty text-destructive">
				{t('catalog.isolatedChangeNotFoundInThis', { selectedId })}
			</p>
		{:else if inspect.pending}
			<p role="status" class="text-xs text-muted-foreground">{t('catalog.lookingForChange', { selectedId })}</p>
		{:else if inspect.refusal}
			<p role="alert" class="rounded-md bg-destructive/10 px-3 py-2 text-xs text-pretty text-destructive">
				{t('catalog.commandRefused', { refusal: inspect.refusal })}
			</p>
		{:else if inspectAnswer && inspectAnswer.ok}
			{@const shown = splitShortened(inspectAnswer.text)}
			<p role="status" class="text-xs text-pretty text-muted-foreground">
				{t('catalog.changeIsKnownToTheEngine', { selectedId })}
			</p>
			<DiffView patch={shown.patch} truncated={shown.truncated} />
		{:else if inspectAnswer}
			<p role="alert" class="rounded-md bg-destructive/10 px-3 py-2 text-xs text-pretty text-destructive">
				{t('catalog.isolatedChangeNotFound', { selectedId, text: inspectAnswer.text })}
			</p>
		{/if}
		{#if changes.length > 0}
			<Button variant="ghost" size="xs" class="self-start" onclick={() => (picked = '')}>
				{t('catalog.changeList')}
			</Button>
		{/if}
	{:else if changes.length === 0}
		<p class="text-muted-foreground">{t('catalog.noIsolatedChangeInThisConversation')}</p>
	{:else}
		<!-- A named, focusable scroll container: keyboard users must be able to scroll
			it (WCAG 2.1.1). The rule only checks the role, so it is suppressed here. -->
		<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
		<div
			role="region"
			aria-label={t('common.isolatedChanges')}
			tabindex="0"
			class="flex max-h-[52vh] flex-col gap-2 overflow-y-auto rounded-sm outline-none focus-visible:ring-2 focus-visible:ring-ring"
		>
			<ul class="flex flex-col gap-2">
				{#each changes as item (item.id)}
					<li>
						<button
							type="button"
							onclick={() => (picked = item.id)}
							class="flex w-full min-w-0 flex-col gap-1.5 rounded-md border border-border bg-background px-2.5 py-2 text-left outline-none hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring"
						>
							<span class="flex min-w-0 items-center gap-2">
								<span class="min-w-0 flex-1 truncate text-sm font-medium">{item.task}</span>
								<Badge variant={changesetVariant(item.state)}>{changesetLabel(item.state)}</Badge>
							</span>
							<span class="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
								<span>{t('common.fileCount', { count: item.files.length })}</span>
								<span class="min-w-0 font-mono break-all">{item.branch}</span>
							</span>
						</button>
					</li>
				{/each}
			</ul>
		</div>
	{/if}
</div>
