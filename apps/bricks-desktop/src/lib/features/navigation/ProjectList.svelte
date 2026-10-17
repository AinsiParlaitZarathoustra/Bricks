<script lang="ts">
	import EllipsisVertical from '@lucide/svelte/icons/ellipsis-vertical';
	import Folder from '@lucide/svelte/icons/folder';
	import Pencil from '@lucide/svelte/icons/pencil';
	import Trash from '@lucide/svelte/icons/trash';
	import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
	import type { Project } from '$lib/ipc/client';
	import { app } from '$lib/state/app.svelte';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Label } from '$lib/components/ui/label';
	import * as Dialog from '$lib/components/ui/dialog';
	import * as AlertDialog from '$lib/components/ui/alert-dialog';
	import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
	import * as Tooltip from '$lib/components/ui/tooltip';
	import { cn } from '$lib/utils';
	import { shortPath } from './format';

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

	function select(project: Project) {
		if (project.missing || project.id === app.currentProjectId) return;
		void app.selectProject(project.id);
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

<ul class="flex flex-col gap-0.5" aria-label="Projets">
	{#each app.projects as project (project.id)}
		{@const current = project.id === app.currentProjectId}
		<li>
			<div
				class={cn(
					'flex items-center gap-1 rounded-md pr-1 transition-colors',
					current ? 'bg-sidebar-accent text-sidebar-accent-foreground' : 'hover:bg-sidebar-accent/60'
				)}
			>
				<button
					type="button"
					class={cn(
						'flex min-w-0 flex-1 items-start gap-2 rounded-md px-2 py-1.5 text-left',
						project.missing && 'text-muted-foreground'
					)}
					aria-current={current ? 'true' : undefined}
					aria-disabled={project.missing ? 'true' : undefined}
					aria-describedby={project.missing ? `${uid}-${project.id}-missing` : undefined}
					title={project.path}
					onclick={() => select(project)}
				>
					<Folder class="mt-0.5 size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
					<span class="flex min-w-0 flex-col">
						<span class="truncate text-[13px] font-medium">{project.name}</span>
						<span class="truncate text-xs text-muted-foreground">{shortPath(project.path)}</span>
					</span>
				</button>

				{#if project.missing}
					<span id={`${uid}-${project.id}-missing`} class="sr-only">Dossier introuvable</span>
					<Tooltip.Root>
						<Tooltip.Trigger>
							{#snippet child({ props })}
								<span
									{...props}
									class="flex size-7 shrink-0 items-center justify-center rounded-md text-warning"
								>
									<TriangleAlert class="size-4" aria-hidden="true" />
								</span>
							{/snippet}
						</Tooltip.Trigger>
						<Tooltip.Content side="right">Dossier introuvable</Tooltip.Content>
					</Tooltip.Root>
				{/if}

				<DropdownMenu.Root>
					<DropdownMenu.Trigger>
						{#snippet child({ props })}
							<Button
								{...props}
								variant="ghost"
								size="icon-sm"
								class="shrink-0 text-muted-foreground"
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
		</li>
	{/each}
</ul>

<Dialog.Root bind:open={renameOpen}>
	<Dialog.Content showCloseButton={false} class="sm:max-w-sm">
		<form onsubmit={submitRename} class="flex flex-col gap-6">
			<Dialog.Header>
				<Dialog.Title>Renommer le projet</Dialog.Title>
				<Dialog.Description>Le nom n’est utilisé que dans Bricks. Le dossier ne change pas.</Dialog.Description>
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
