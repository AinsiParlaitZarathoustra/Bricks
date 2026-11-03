<script lang="ts">
	// « Dossier de travail introuvable »: shown while an action waits for a project
	// whose folder is missing. Choosing its new place moves the project and its
	// conversations there, then the action goes on. Mounted once, by App.svelte.
	import { t } from '$lib/i18n/i18n.svelte';
	import * as AlertDialog from '$lib/components/ui/alert-dialog';
	import { Button } from '$lib/components/ui/button';
	import { app } from '$lib/state/app.svelte';

	const project = $derived(
		app.folderRequest ? (app.projects.find((p) => p.id === app.folderRequest?.projectId) ?? null) : null
	);
	let choosing = $state(false);

	async function choose() {
		if (choosing) return;
		choosing = true;
		try {
			await app.chooseFolder();
		} finally {
			choosing = false;
		}
	}

	function onOpenChange(open: boolean) {
		if (!open) app.cancelFolderRequest();
	}
</script>

<AlertDialog.Root open={app.folderRequest !== null} {onOpenChange}>
	<AlertDialog.Content>
		<AlertDialog.Header>
			<AlertDialog.Title>{t('navigation.workingFolderNotFound')}</AlertDialog.Title>
			{#if project}
				<AlertDialog.Description class="break-all">
					{project.name} · <span class="font-mono text-xs">{project.path}</span>
				</AlertDialog.Description>
			{/if}
		</AlertDialog.Header>
		<AlertDialog.Footer>
			<AlertDialog.Cancel>{t('common.cancel')}</AlertDialog.Cancel>
			<Button onclick={choose} disabled={choosing}>{t('navigation.chooseAFolder')}</Button>
		</AlertDialog.Footer>
	</AlertDialog.Content>
</AlertDialog.Root>
