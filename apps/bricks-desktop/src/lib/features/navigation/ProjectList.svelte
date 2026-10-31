<script lang="ts">
	import EllipsisVertical from '@lucide/svelte/icons/ellipsis-vertical';
	import Folder from '@lucide/svelte/icons/folder';
	import FolderOpen from '@lucide/svelte/icons/folder-open';
	import FolderX from '@lucide/svelte/icons/folder-x';
	import Pencil from '@lucide/svelte/icons/pencil';
	import Plus from '@lucide/svelte/icons/plus';
	import Trash from '@lucide/svelte/icons/trash';
	import type { Project } from '$lib/ipc/client';
	import { app } from '$lib/state/app.svelte';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Label } from '$lib/components/ui/label';
	import * as Dialog from '$lib/components/ui/dialog';
	import * as AlertDialog from '$lib/components/ui/alert-dialog';
	import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
	import { cn } from '$lib/utils';
	import ConversationList from './ConversationList.svelte';

	/** Conversations listed under a project before « Afficher plus ». */
	const SHOWN_PER_PROJECT = 3;

	const uid = $props.id();

	// The dialogs are rendered once, outside the menus, so they outlive the menu closing.
	let renameTarget = $state<Project | null>(null);
	let renameOpen = $state(false);
	let renameName = $state('');
	let renameError = $state<string | null>(null);
	let renaming = $state(false);

	let removeTarget = $state<Project | null>(null);
	let removeOpen = $state(false);

	// Set when a menu item opens a dialog: the menu must not hand focus back to its trigger.
	let keepFocus = false;

	function toggle(project: Project) {
		if (app.collapsedProjects.has(project.id)) app.collapsedProjects.delete(project.id);
		else app.collapsedProjects.add(project.id);
	}

	// A project whose folder is missing asks for its new place first (in app.newConversation).
	function startConversation(project: Project) {
		if (!app.hasProviders) return;
		app.collapsedProjects.delete(project.id);
		void app.newConversation(project.id);
	}

	function onMenuClose(event: Event) {
		if (keepFocus) event.preventDefault();
		keepFocus = false;
	}

	function openRename(project: Project) {
		keepFocus = true;
		renameTarget = project;
		renameName = project.name;
		renameError = null;
		renameOpen = true;
	}

	function openRemove(project: Project) {
		keepFocus = true;
		removeTarget = project;
		removeOpen = true;
	}

	async function submitRename(event: SubmitEvent) {
		event.preventDefault();
		const target = renameTarget;
		if (!target || renaming) return;
		const name = renameName.trim();
		if (!name) {
			renameError = 'Donnez un nom au projet.';
			return;
		}
		if (name === target.name) {
			renameOpen = false;
			return;
		}
		renaming = true;
		const before = app.lastError;
		await app.renameProject(target.id, name);
		renaming = false;
		// renameProject reports failures through app.lastError (shown as a toast): keep the dialog open.
		if (app.lastError === before) renameOpen = false;
	}

	function confirmRemove() {
		const target = removeTarget;
		if (target) void app.removeProject(target.id);
	}
</script>

<ul class="flex flex-col gap-2" aria-label="Projets">
	{#each app.folders as project (project.id)}
		{@const expanded = !app.collapsedProjects.has(project.id)}
		{@const drafting = project.id === app.currentProjectId && app.currentSessionId === null}
		<li>
			<div
				class={cn(
					'group flex items-center gap-1 rounded-md pr-1 transition-colors',
					drafting ? 'bg-sidebar-accent text-sidebar-accent-foreground' : 'hover:bg-sidebar-accent/60'
				)}
			>
				<button
					type="button"
					class="flex min-w-0 flex-1 items-center gap-2 rounded-md px-2 py-1.5 text-left"
					aria-expanded={expanded}
					aria-describedby={project.missing ? `${uid}-${project.id}-missing` : undefined}
					title={project.path}
					onclick={() => toggle(project)}
				>
					{#if project.missing}
						<FolderX class="size-4 shrink-0 text-destructive" aria-hidden="true" />
					{:else if expanded}
						<FolderOpen class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
					{:else}
						<Folder class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
					{/if}
					<span class="truncate text-compact font-medium">{project.name}</span>
				</button>
				{#if project.missing}
					<span id={`${uid}-${project.id}-missing`} class="sr-only">Dossier introuvable</span>
				{/if}

				<Button
					variant="ghost"
					size="icon-sm"
					class="shrink-0 text-muted-foreground opacity-0 group-focus-within:opacity-100 group-hover:opacity-100"
					aria-label={`Nouvelle conversation dans ${project.name}`}
					onclick={() => startConversation(project)}
				>
					<Plus class="size-4" aria-hidden="true" />
				</Button>

				<DropdownMenu.Root>
					<DropdownMenu.Trigger>
						{#snippet child({ props })}
							<Button
								{...props}
								variant="ghost"
								size="icon-sm"
								class="shrink-0 text-muted-foreground opacity-0 group-focus-within:opacity-100 group-hover:opacity-100 data-[state=open]:opacity-100"
								aria-label={`Actions pour le projet ${project.name}`}
							>
								<EllipsisVertical class="size-4" aria-hidden="true" />
							</Button>
						{/snippet}
					</DropdownMenu.Trigger>
					<DropdownMenu.Content align="end" class="w-44" onCloseAutoFocus={onMenuClose}>
						<DropdownMenu.Item onSelect={() => openRename(project)}>
							<Pencil class="size-4" aria-hidden="true" />
							Renommer…
						</DropdownMenu.Item>
						<DropdownMenu.Separator />
						<DropdownMenu.Item variant="destructive" onSelect={() => openRemove(project)}>
							<Trash class="size-4" aria-hidden="true" />
							Retirer de Bricks
						</DropdownMenu.Item>
					</DropdownMenu.Content>
				</DropdownMenu.Root>
			</div>
			{#if expanded && app.sessionsOf(project.id).length > 0}
				<div class="mt-0.5">
					<ConversationList
						projectId={project.id}
						label={`Conversations de ${project.name}`}
						limit={SHOWN_PER_PROJECT}
						nested
					/>
				</div>
			{/if}
		</li>
	{/each}
</ul>

<Dialog.Root bind:open={renameOpen}>
	<Dialog.Content showCloseButton={false} class="sm:max-w-sm">
		<form onsubmit={submitRename} class="flex flex-col gap-6">
			<Dialog.Header>
				<Dialog.Title>Renommer le projet</Dialog.Title>
			</Dialog.Header>
			<div class="flex flex-col gap-2">
				<Label for={`${uid}-name`}>Nom du projet</Label>
				<Input
					id={`${uid}-name`}
					bind:value={renameName}
					maxlength={120}
					autocomplete="off"
					aria-invalid={renameError ? 'true' : undefined}
					aria-describedby={renameError ? `${uid}-error` : undefined}
				/>
				{#if renameError}
					<p id={`${uid}-error`} class="text-xs text-destructive" role="alert">{renameError}</p>
				{/if}
			</div>
			<Dialog.Footer>
				<Button type="button" variant="outline" onclick={() => (renameOpen = false)}>Annuler</Button>
				<Button type="submit" disabled={renaming}>Enregistrer</Button>
			</Dialog.Footer>
		</form>
	</Dialog.Content>
</Dialog.Root>

<AlertDialog.Root bind:open={removeOpen}>
	<AlertDialog.Content>
		<AlertDialog.Header>
			<AlertDialog.Title>Retirer « {removeTarget?.name ?? ''} » de Bricks ?</AlertDialog.Title>
			<AlertDialog.Description>
				Le projet disparaît de la liste de Bricks. Les fichiers du dossier et les conversations ne sont pas
				supprimés.
			</AlertDialog.Description>
		</AlertDialog.Header>
		<AlertDialog.Footer>
			<AlertDialog.Cancel>Annuler</AlertDialog.Cancel>
			<AlertDialog.Action onclick={confirmRemove}>Retirer</AlertDialog.Action>
		</AlertDialog.Footer>
	</AlertDialog.Content>
</AlertDialog.Root>
