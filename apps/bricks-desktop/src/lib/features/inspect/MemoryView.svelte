<script lang="ts">
	// The long-term memory (`/memory`, `/memory global`, the settings): the entries of the
	// project's or the global memory, 50 per page, searched and filtered by type. Reading
	// works whether the memory is on or off. A note is one entry, never also an exchange.
	import { t, localeTag } from '$lib/i18n/i18n.svelte';
	import type { InspectorState } from '$lib/state/ui.svelte';
	import type { MemoryEntry, MemoryEntryFilter, MemoryEntryPage, MemoryScope } from '$lib/ipc/client';
	import * as ipc from '$lib/ipc/client';
	import { app } from '$lib/state/app.svelte';
	import * as Tabs from '$lib/components/ui/tabs';
	import * as Select from '$lib/components/ui/select';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Skeleton } from '$lib/components/ui/skeleton';
	import { formatInt } from './format';

	let { args }: { args: InspectorState['args'] } = $props();

	const PAGE = 50;
	const uid = $props.id();

	/** The open project, when it has a memory (not the conversations of no project). */
	const project = $derived(app.currentProject && !app.currentProject.chats ? app.currentProject : null);

	let scope = $derived<MemoryScope>(args.scope === 'global' || !project ? 'global' : 'project');
	let filter = $state<MemoryEntryFilter>('all');
	let query = $state('');
	let offset = $state(0);
	let page = $state<MemoryEntryPage | null>(null);
	let loadError = $state<string | null>(null);

	const FILTERS: { value: MemoryEntryFilter; label: string }[] = [
		{ value: 'all', get label() { return t('inspect.all') } },
		{ value: 'notes', get label() { return t('inspect.notes') } },
		{ value: 'exchanges', get label() { return t('inspect.exchanges') } },
		{ value: 'facts', get label() { return t('inspect.facts') } },
		{ value: 'fact_history', get label() { return t('inspect.factHistory') } }
	];
	const KIND: Record<MemoryEntry['kind'], string> = {
		get note() { return t('common.note') },
		get exchange() { return t('inspect.exchange') },
		get fact() { return t('inspect.fact') }
	};
	const ROLE: Record<string, string> = {
		user: 'message',
		get assistant() { return t('inspect.answer') },
		get tool() { return t('inspect.memoryTool') }
	};
	const STATUS: Record<string, string> = {
		get active() { return t('inspect.memoryActive') },
		get proposed() { return t('inspect.proposed') },
		get superseded() { return t('inspect.replaced') },
		get contested() { return t('inspect.disputed') },
		get retracted() { return t('inspect.withdrawn') },
		get unsupported() { return t('inspect.noSource') }
	};

	const filterLabel = $derived(FILTERS.find((f) => f.value === filter)?.label ?? '');
	const factsShown = $derived(filter === 'facts' || filter === 'fact_history');

	// Reads the page for the current scope, filter, search and offset. The search waits for
	// typing to pause; an older answer never replaces a newer one.
	let request = 0;
	$effect(() => {
		const read = { scope, filter, query: query.trim(), offset, projectId: project?.id ?? null };
		const id = ++request;
		const timer = setTimeout(async () => {
			try {
				const result = await ipc.memoryEntries(
					read.scope,
					read.scope === 'project' ? read.projectId : null,
					read.filter,
					read.query,
					read.offset,
					PAGE
				);
				if (id !== request) return;
				page = result;
				loadError = null;
			} catch (e) {
				if (id !== request) return;
				loadError = ipc.errorMessage(e);
			}
		}, read.query ? 250 : 0);
		return () => clearTimeout(timer);
	});

	function choose(next: () => void) {
		next();
		offset = 0;
	}

	function when(at: number): string {
		return new Date(at).toLocaleString(localeTag(), { dateStyle: 'medium', timeStyle: 'short' });
	}

	/** The title of a conversation of the loaded lists, when known. */
	function conversation(sessionId: string): string | null {
		for (const list of app.sessionsByProject.values()) {
			const found = list.find((s) => s.id === sessionId);
			if (found) return found.title || t('common.untitled');
		}
		return null;
	}

	/** Where an entry comes from: its project (global memory), its conversation. */
	function source(e: MemoryEntry): string {
		const parts: string[] = [];
		if (scope === 'global' && e.project_id) parts.push(t('inspect.project', { projectId: e.project_name ?? e.project_id }));
		if (e.session_id) parts.push(conversation(e.session_id) ?? t('inspect.closedConversation'));
		return parts.join(' · ');
	}
</script>

<div class="flex min-w-0 flex-col gap-4">
	<div class="flex flex-wrap items-center gap-2">
		<Tabs.Root value={scope} onValueChange={(v) => choose(() => (scope = v as MemoryScope))}>
			<Tabs.List aria-label={t('inspect.memoryShown')}>
				<Tabs.Trigger value="project" disabled={!project}>{t('common.project')}</Tabs.Trigger>
				<Tabs.Trigger value="global">{t('inspect.global')}</Tabs.Trigger>
			</Tabs.List>
		</Tabs.Root>
		<Input
			type="search"
			placeholder={t('common.search')}
			aria-label={t('inspect.searchTheMemory')}
			class="h-8 min-w-40 flex-1 text-ui"
			value={query}
			oninput={(event) => choose(() => (query = event.currentTarget.value))}
		/>
		<Select.Root
			type="single"
			value={filter}
			onValueChange={(v) => choose(() => (filter = v as MemoryEntryFilter))}
		>
			<Select.Trigger id={`${uid}-filter`} aria-label={t('inspect.type')} class="h-8 w-48 text-ui">
				<span class="truncate">{filterLabel}</span>
			</Select.Trigger>
			<Select.Content>
				{#each FILTERS as f (f.value)}
					<Select.Item value={f.value} label={f.label}>{f.label}</Select.Item>
				{/each}
			</Select.Content>
		</Select.Root>
	</div>

	{#if factsShown}
		<p class="text-caption text-muted-foreground">{t('inspect.noFactIsExtractedAutomatically')}</p>
	{/if}

	{#if loadError !== null}
		<p role="alert" class="text-ui text-pretty text-destructive">{loadError}</p>
	{:else if page === null}
		<div class="flex flex-col gap-2">
			<Skeleton class="h-14 w-full" />
			<Skeleton class="h-14 w-full" />
		</div>
	{:else if page.entries.length === 0}
		<p class="text-ui text-muted-foreground">{t('inspect.noMemory')}</p>
	{:else}
		<ul class="flex flex-col divide-y divide-border rounded-lg border border-border">
			{#each page.entries as entry (entry.id)}
				<li class="flex min-w-0 flex-col gap-1.5 px-3 py-2.5">
					<div class="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-caption text-muted-foreground">
						<Badge variant="secondary">{KIND[entry.kind]}</Badge>
						{#if entry.kind === 'exchange' && entry.role}
							<span>{ROLE[entry.role] ?? entry.role}</span>
						{/if}
						{#if entry.status}
							<span>{STATUS[entry.status] ?? entry.status}</span>
						{/if}
						<time datetime={new Date(entry.at).toISOString()}>{when(entry.at)}</time>
						{#if source(entry)}
							<span class="min-w-0 truncate">{source(entry)}</span>
						{/if}
					</div>
					<p class="text-ui break-words whitespace-pre-wrap">{entry.text}</p>
					{#if entry.evidence.length > 0}
						<ul class="flex flex-col gap-1 border-l-2 border-border pl-2 text-caption text-muted-foreground">
							{#each entry.evidence as proof, i (i)}
								<li class="break-words">{t('common.quoted', { text: proof.quote })}</li>
							{/each}
						</ul>
					{/if}
				</li>
			{/each}
		</ul>
		<div class="flex items-center justify-between gap-2 text-caption text-muted-foreground">
			<span class="tabular-nums">
				{t('inspect.of', { value: formatInt(offset + 1), entries: formatInt(offset + page.entries.length), total: formatInt(page.total) })}
			</span>
			<div class="flex gap-2">
				<Button
					variant="ghost"
					size="sm"
					disabled={offset === 0}
					onclick={() => (offset = Math.max(0, offset - PAGE))}
				>
					{t('common.previous')}
				</Button>
				<Button
					variant="ghost"
					size="sm"
					disabled={offset + PAGE >= page.total}
					onclick={() => (offset += PAGE)}
				>
					{t('common.next')}
				</Button>
			</div>
		</div>
	{/if}
</div>
