<script lang="ts">
	// Approval policy of the desktop's tools (`Preferences.tool_approval`). The engine applies it.
	import { app } from '$lib/state/app.svelte';
	import * as Select from '$lib/components/ui/select';
	import { Label } from '$lib/components/ui/label';
	import { TOOL_APPROVAL_OPTIONS, isToolApproval } from './format';

	const uid = $props.id();

	const selected = $derived(TOOL_APPROVAL_OPTIONS.find((o) => o.value === app.prefs.tool_approval));

	/**
	 * `lastError.at` when the last change was sent (0 when none was). An error stamped later than
	 * that is the failure of this change; the store rolls the saved value back.
	 */
	let sentAfter = $state<number | null>(null);
	const failure = $derived(
		sentAfter !== null && app.lastError && app.lastError.at !== sentAfter ? app.lastError.message : null
	);

	/** Saves the policy at once (the select shows the pick; the store rolls it back on failure). */
	function choose(next: string | undefined) {
		if (!isToolApproval(next) || next === app.prefs.tool_approval) return;
		sentAfter = app.lastError?.at ?? 0;
		app.setPrefs({ tool_approval: next });
	}
</script>

<div class="flex flex-col gap-3">
	<Label for={`${uid}-approval`} class="text-sm font-semibold text-foreground">Approbation des outils</Label>
	<Select.Root type="single" value={app.prefs.tool_approval} onValueChange={choose}>
		<Select.Trigger id={`${uid}-approval`} class="w-full max-w-72" aria-describedby={`${uid}-effect`}>
			<span class="truncate">{selected?.label ?? app.prefs.tool_approval}</span>
		</Select.Trigger>
		<Select.Content>
			{#each TOOL_APPROVAL_OPTIONS as option (option.value)}
				<Select.Item value={option.value} label={option.label}>{option.label}</Select.Item>
			{/each}
		</Select.Content>
	</Select.Root>
	{#if selected}
		<p id={`${uid}-effect`} class="text-sm text-pretty text-muted-foreground">{selected.effect}</p>
	{/if}
	{#if failure}
		<p role="alert" class="text-sm text-pretty text-destructive">
			Le réglage n’a pas été enregistré : {failure}
		</p>
	{/if}
</div>
