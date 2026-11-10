<script lang="ts">
	import { t } from '$lib/i18n/i18n.svelte';
	import { tick } from 'svelte';
	import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';
	import EllipsisVertical from '@lucide/svelte/icons/ellipsis-vertical';
	import Folder from '@lucide/svelte/icons/folder';
	import Pencil from '@lucide/svelte/icons/pencil';
	import Pin from '@lucide/svelte/icons/pin';
	import PinOff from '@lucide/svelte/icons/pin-off';
	import Trash from '@lucide/svelte/icons/trash';
	import type { SessionItem } from '$lib/ipc/client';
	import { app } from '$lib/state/app.svelte';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import * as AlertDialog from '$lib/components/ui/alert-dialog';
	import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
	import StatusDot from '$lib/layout/StatusDot.svelte';
	import { cn } from '$lib/utils';
	import { displayTitle } from './format';

	interface Props {
		/** The project whose conversations are listed (pinned ones left out). */
		projectId?: string;
		/** An explicit list instead, shown as given (the pinned conversations). */
		sessions?: SessionItem[];
		/** Accessible name of the list. */
		label: string;
		/** Conversations shown before « Afficher plus » (all when absent). */
		limit?: number;
		/** Indented under its project's row. */
		nested?: boolean;
	}

	let { projectId, sessions, label, limit, nested = false }: Props = $props();

	let showAll = $state(false);

	const sorted = $derived(
		sessions ??
			[...app.sessionsOf(projectId ?? '')]
				.filter((s) => !app.isPinned(s.id))
				.sort((a, b) => b.updated_at - a.updated_at)
	);

	/** The project a pinned conversation belongs to, shown under its title (none for the chats project). */
	function projectName(session: SessionItem): string | null {
		const project = app.projects.find((p) => p.id === session.project_id);
		return project && !project.chats ? project.name : null;
	}
	// The shown conversation is always listed, even past the limit.
	const visible = $derived(
		showAll || limit === undefined
			? sorted
			: sorted.filter((s, i) => i < limit || s.id === app.currentSessionId)
	);

	// The list element, kept for keyboard navigation and focus restoration (not rendered from).
	let listEl: HTMLElement | null = null;

	function trackList(element: HTMLElement) {
		listEl = element;
		return () => {
			listEl = null;
		};
	}

	// Inline rename: the id of the conversation being renamed, and its draft title.
	let editingId = $state<string | null>(null);
	let draft = $state('');

	let deleteTarget = $state<SessionItem | null>(null);
	let deleteOpen = $state(false);

	// Set when a menu item starts a rename or a delete: the menu must not hand focus back to its trigger.
	let keepFocus = false;

	// Focuses and selects the rename field as soon as it is mounted.
	function focusAndSelect(element: HTMLInputElement) {
		element.focus();
		element.select();
	}

	function onMenuClose(event: Event) {
		if (keepFocus) event.preventDefault();
		keepFocus = false;
	}

	function startRename(session: SessionItem) {
		editingId = session.id;
		draft = session.title;
	}

	async function focusItem(id: string) {
		await tick();
		listEl?.querySelector<HTMLElement>(`[data-session-item="${CSS.escape(id)}"]`)?.focus();
	}

	/** Ends the rename. Enter and Escape give focus back to the row; a blur does not steal it. */
	async function endRename(save: boolean, refocus: boolean) {
		const id = editingId;
		if (!id) return;
		const session = sorted.find((s) => s.id === id);
		const title = draft.trim();
		editingId = null;
		if (save && session && title && title !== session.title) {
			await app.renameSession(id, title);
		}
		if (refocus) await focusItem(id);
	}

	function onRenameKey(event: KeyboardEvent) {
		if (event.isComposing) return;
		if (event.key === 'Enter') {
			event.preventDefault();
			void endRename(true, true);
		} else if (event.key === 'Escape') {
			event.preventDefault();
			event.stopPropagation();
			void endRename(false, true);
		}
	}

	function onItemKeydown(event: KeyboardEvent, session: SessionItem) {
		if (event.key === 'F2') {
			event.preventDefault();
			startRename(session);
			return;
		}
		if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
		const items = Array.from(listEl?.querySelectorAll<HTMLElement>('[data-session-item]') ?? []);
		const index = items.indexOf(event.currentTarget as HTMLElement);
		if (index < 0) return;
		event.preventDefault();
		let next = index;
		if (event.key === 'ArrowDown') next = Math.min(index + 1, items.length - 1);
		else if (event.key === 'ArrowUp') next = Math.max(index - 1, 0);
		else if (event.key === 'Home') next = 0;
		else next = items.length - 1;
		items[next]?.focus();
	}

	/**
	 * Marks a title clipped by its row (`data-clipped`), so only long titles fade out.
	 * Re-checked when the row is resized and when the title changes.
	 */
	function fadeWhenClipped(title: string) {
		return (element: HTMLElement) => {
			void title;
			const update = () => element.toggleAttribute('data-clipped', element.scrollWidth > element.clientWidth + 0.5);
			const observer = new ResizeObserver(update);
			observer.observe(element);
			update();
			return () => observer.disconnect();
		};
	}

	function askDelete(session: SessionItem) {
		keepFocus = true;
		deleteTarget = session;
		deleteOpen = true;
	}

	function confirmDelete() {
		const target = deleteTarget;
		if (target) void app.deleteSession(target.id);
	}
</script>

<ul {@attach trackList} class="flex flex-col gap-0.5" aria-label={label}>
	{#each visible as session (session.id)}
		{@const current = session.id === app.currentSessionId}
		{@const projectLabel = sessions ? projectName(session) : null}
		<li>
			<div
				class={cn(
					'group relative flex items-center rounded-lg transition-colors',
					current
						? 'bg-background text-foreground shadow-[0_1px_2px_rgb(0_0_0/0.06),0_0_0_1px_var(--border-subtle)]'
						: 'hover:bg-element-hover'
				)}
			>
				{#if editingId === session.id}
					<div class={cn('min-w-0 flex-1 py-1 pr-9', nested ? 'pl-7' : 'pl-1.5')}>
						<Input
							{@attach focusAndSelect}
							bind:value={draft}
							aria-label={t('navigation.newConversationTitle')}
							maxlength={200}
							autocomplete="off"
							class="h-8 text-compact"
							onkeydown={onRenameKey}
							onblur={() => void endRename(true, false)}
						/>
					</div>
				{:else}
					<button
						type="button"
						data-session-item={session.id}
						class={cn(
							'flex min-w-0 flex-1 items-center gap-2 rounded-lg py-1.5 pr-2 text-left group-focus-within:pr-9 group-hover:pr-9 group-has-[[data-state=open]]:pr-9',
							nested ? 'pl-8' : 'pl-2'
						)}
						aria-current={current ? 'true' : undefined}
						title={displayTitle(session.title)}
						onclick={() => void app.openSession(session.id)}
						ondblclick={() => {
							keepFocus = false;
							startRename(session);
						}}
						onkeydown={(event) => onItemKeydown(event, session)}
					>
						<span class="flex min-w-0 flex-1 flex-col">
							<span
								{@attach fadeWhenClipped(session.title)}
								class="title-fade min-w-0 overflow-hidden text-compact whitespace-nowrap"
								>{displayTitle(session.title)}</span
							>
							{#if projectLabel}
								<span class="flex min-w-0 items-center gap-1 text-caption text-muted-foreground">
									<Folder class="size-3 shrink-0" aria-hidden="true" />
									<span class="truncate">{projectLabel}</span>
								</span>
							{/if}
						</span>
						{#if session.running}
							<StatusDot status="running" label={t('navigation.conversationRunning')} />
						{/if}
					</button>
				{/if}

				<DropdownMenu.Root>
					<DropdownMenu.Trigger>
						{#snippet child({ props })}
							<Button
								{...props}
								variant="ghost"
								size="icon-sm"
								class="absolute top-1/2 right-1 shrink-0 -translate-y-1/2 text-muted-foreground opacity-0 group-focus-within:opacity-100 group-hover:opacity-100 data-[state=open]:opacity-100"
								aria-label={t('navigation.actionsForConversation', { title: displayTitle(session.title) })}
							>
								<EllipsisVertical class="size-4" aria-hidden="true" />
							</Button>
						{/snippet}
					</DropdownMenu.Trigger>
					<DropdownMenu.Content align="end" class="w-44" onCloseAutoFocus={onMenuClose}>
						<DropdownMenu.Item
							onSelect={() => {
								keepFocus = true;
								startRename(session);
							}}
						>
							<Pencil class="size-4" aria-hidden="true" />
							{t('navigation.rename')}
						</DropdownMenu.Item>
						<DropdownMenu.Item onSelect={() => app.togglePin(session.id)}>
							{#if app.isPinned(session.id)}
								<PinOff class="size-4" aria-hidden="true" />
								{t('navigation.unpin')}
							{:else}
								<Pin class="size-4" aria-hidden="true" />
								{t('navigation.pin')}
							{/if}
						</DropdownMenu.Item>
						<DropdownMenu.Separator />
						<DropdownMenu.Item variant="destructive" onSelect={() => askDelete(session)}>
							<Trash class="size-4" aria-hidden="true" />
							{t('common.delete')}
						</DropdownMenu.Item>
					</DropdownMenu.Content>
				</DropdownMenu.Root>
			</div>
		</li>
	{/each}
	{#if limit !== undefined && sorted.length > limit && (showAll || visible.length < sorted.length)}
		<li>
			<button
				type="button"
				class={cn(
					'flex w-full items-center gap-1 rounded-lg py-1.5 pr-2 text-left text-xs font-medium text-foreground/70 transition-colors hover:bg-element-hover hover:text-foreground',
					nested ? 'pl-8' : 'pl-2'
				)}
				aria-expanded={showAll}
				onclick={() => (showAll = !showAll)}
			>
				{showAll ? t('navigation.showLess') : t('navigation.showMore')}
				<ChevronDownIcon
					class={cn('size-3.5 transition-transform', showAll && 'rotate-180')}
					aria-hidden="true"
				/>
			</button>
		</li>
	{/if}
</ul>

<AlertDialog.Root bind:open={deleteOpen}>
	<AlertDialog.Content>
		<AlertDialog.Header>
			<AlertDialog.Title>{t('navigation.delete', { value: displayTitle(deleteTarget?.title ?? '') })}</AlertDialog.Title>
			<AlertDialog.Description>
				{#if deleteTarget?.running}
					{t('navigation.theConversationIsRunningItWill')}
				{/if}
				{t('navigation.thisIsPermanentTheConversationAnd')}
			</AlertDialog.Description>
		</AlertDialog.Header>
		<AlertDialog.Footer>
			<AlertDialog.Cancel>{t('common.cancel')}</AlertDialog.Cancel>
			<AlertDialog.Action variant="destructive" onclick={confirmDelete}>{t('common.delete')}</AlertDialog.Action>
		</AlertDialog.Footer>
	</AlertDialog.Content>
</AlertDialog.Root>

<style>
	/* A clipped title fades out over its last pixels instead of ending on an ellipsis. */
	.title-fade:global([data-clipped]) {
		--fade: 24px;
		-webkit-mask-image: linear-gradient(to right, #000 calc(100% - var(--fade)), transparent);
		mask-image: linear-gradient(to right, #000 calc(100% - var(--fade)), transparent);
	}
</style>
