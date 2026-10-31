<script lang="ts">
	import { tick } from 'svelte';
	import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';
	import EllipsisVertical from '@lucide/svelte/icons/ellipsis-vertical';
	import Pencil from '@lucide/svelte/icons/pencil';
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
		projectId: string;
		/** Accessible name of the list. */
		label: string;
		/** Conversations shown before « Afficher plus » (all when absent). */
		limit?: number;
		/** Indented under its project's row. */
		nested?: boolean;
	}

	let { projectId, label, limit, nested = false }: Props = $props();

	let showAll = $state(false);

	const sorted = $derived([...app.sessionsOf(projectId)].sort((a, b) => b.updated_at - a.updated_at));
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
		<li>
			<div
				class={cn(
					'group relative flex items-center rounded-md transition-colors',
					current ? 'bg-sidebar-accent text-sidebar-accent-foreground' : 'hover:bg-sidebar-accent/60'
				)}
			>
				{#if editingId === session.id}
					<div class={cn('min-w-0 flex-1 py-1 pr-9', nested ? 'pl-7' : 'pl-1.5')}>
						<Input
							{@attach focusAndSelect}
							bind:value={draft}
							aria-label="Nouveau titre de la conversation"
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
							'flex min-w-0 flex-1 items-center gap-2 rounded-md py-1.5 pr-2 text-left group-focus-within:pr-9 group-hover:pr-9 group-has-[[data-state=open]]:pr-9',
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
						<span
							{@attach fadeWhenClipped(session.title)}
							class="title-fade min-w-0 flex-1 overflow-hidden text-compact whitespace-nowrap"
							>{displayTitle(session.title)}</span
						>
						{#if session.running}
							<StatusDot status="running" label="Conversation en cours" />
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
								aria-label={`Actions pour la conversation ${displayTitle(session.title)}`}
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
							Renommer
						</DropdownMenu.Item>
						<DropdownMenu.Separator />
						<DropdownMenu.Item variant="destructive" onSelect={() => askDelete(session)}>
							<Trash class="size-4" aria-hidden="true" />
							Supprimer
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
					'flex w-full items-center gap-1 rounded-md py-1.5 pr-2 text-left text-xs font-medium text-foreground/70 transition-colors hover:bg-sidebar-accent/60 hover:text-foreground',
					nested ? 'pl-8' : 'pl-2'
				)}
				aria-expanded={showAll}
				onclick={() => (showAll = !showAll)}
			>
				{showAll ? 'Afficher moins' : 'Afficher plus'}
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
			<AlertDialog.Title>Supprimer « {displayTitle(deleteTarget?.title ?? '')} » ?</AlertDialog.Title>
			<AlertDialog.Description>
				{#if deleteTarget?.running}
					La conversation est en cours : elle sera annulée avant sa suppression.
				{/if}
				Cette action est définitive. La conversation et ses messages seront supprimés et ne pourront pas être
				récupérés.
			</AlertDialog.Description>
		</AlertDialog.Header>
		<AlertDialog.Footer>
			<AlertDialog.Cancel>Annuler</AlertDialog.Cancel>
			<AlertDialog.Action variant="destructive" onclick={confirmDelete}>Supprimer</AlertDialog.Action>
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
