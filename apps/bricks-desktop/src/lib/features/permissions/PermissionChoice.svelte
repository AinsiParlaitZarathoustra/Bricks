<script lang="ts">
	// The first choice of permissions, above the composer, while the conversation's
	// policy is not chosen. The choice applies to this conversation and becomes the
	// one new conversations start from. CUSTOM opens the configuration to edit it.
	import ShieldQuestion from '@lucide/svelte/icons/shield-question';
	import { Button } from '$lib/components/ui/button';
	import type { PermissionProfile } from '$lib/ipc/engine';
	import { app } from '$lib/state/app.svelte';
	import type { Conversation } from '$lib/state/conversation.svelte';
	import { ui } from '$lib/state/ui.svelte';
	import { PROFILE_OPTIONS, withProfile } from './policy';

	let { conversation }: { conversation: Conversation | null } = $props();

	const policy = $derived(conversation?.permissions?.policy ?? null);
	let sending = $state(false);
	let refusal = $state<string | null>(null);

	async function choose(profile: PermissionProfile) {
		if (!conversation || !policy || sending) return;
		sending = true;
		refusal = await app.setPermissions(conversation.sessionId, withProfile(policy, profile), true);
		sending = false;
		if (!refusal && profile === 'custom') ui.open('config');
	}
</script>

{#if conversation && policy && !policy.chosen}
	<section
		aria-labelledby="permission-choice"
		class="mx-auto flex w-full max-w-3xl flex-wrap items-center gap-x-3 gap-y-2 px-4 pt-2 text-sm"
	>
		<ShieldQuestion class="size-4 shrink-0 text-warning" aria-hidden="true" />
		<h2 id="permission-choice" class="font-medium">Permissions</h2>
		<div class="flex flex-wrap gap-1.5">
			{#each PROFILE_OPTIONS as option (option.value)}
				<Button
					size="sm"
					variant="outline"
					disabled={sending}
					title={option.detail}
					onclick={() => choose(option.value)}
				>
					{option.label}
					<span class="text-muted-foreground">{option.detail}</span>
				</Button>
			{/each}
		</div>
		{#if refusal}
			<p role="alert" class="basis-full text-xs break-words text-destructive">{refusal}</p>
		{/if}
	</section>
{/if}
