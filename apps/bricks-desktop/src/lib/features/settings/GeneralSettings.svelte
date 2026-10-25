<script lang="ts">
	// The permission policy new conversations start from (`~/.bricks/permissions.json`).
	// Open conversations keep theirs.
	import { onMount } from 'svelte';
	import DiagramSettings from './DiagramSettings.svelte';
	import PolicyEditor from '$lib/features/permissions/PolicyEditor.svelte';
	import { unchosenPolicy } from '$lib/features/permissions/policy';
	import { errorMessage, permissionsDefaultGet, permissionsDefaultSet } from '$lib/ipc/client';
	import type { SessionPolicy } from '$lib/ipc/engine';

	let policy = $state<SessionPolicy | null>(null);
	let loadError = $state<string | null>(null);
	let saveError = $state<string | null>(null);
	let saving = $state(false);

	onMount(() => {
		void permissionsDefaultGet()
			.then((read) => {
				policy = read.policy ?? unchosenPolicy();
				loadError = read.error;
			})
			.catch((e) => {
				policy = unchosenPolicy();
				loadError = errorMessage(e);
			});
	});

	/** Saves the new default; the editor shows it only once it is saved. */
	async function save(next: SessionPolicy) {
		saving = true;
		saveError = null;
		try {
			await permissionsDefaultSet(next);
			policy = next;
			loadError = null;
		} catch (e) {
			saveError = errorMessage(e);
		} finally {
			saving = false;
		}
	}
</script>

<div class="flex flex-col gap-6">
	<section aria-labelledby="default-permissions" class="flex flex-col gap-3">
		<h3 id="default-permissions" class="text-[13px] font-semibold text-foreground">
			Permissions des nouvelles conversations
		</h3>
		{#if policy}
			<PolicyEditor {policy} disabled={saving} onchange={save} />
		{/if}
		{#if loadError}
			<p role="alert" class="text-[13px] text-pretty text-destructive">
				Lecture impossible : {loadError}
			</p>
		{/if}
		{#if saveError}
			<p role="alert" class="text-[13px] text-pretty text-destructive">
				Le réglage n’a pas été enregistré : {saveError}
			</p>
		{/if}
	</section>

	<DiagramSettings />
</div>
