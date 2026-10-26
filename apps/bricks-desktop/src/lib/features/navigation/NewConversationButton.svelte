<script lang="ts">
	import SquarePen from '@lucide/svelte/icons/square-pen';
	import { Button } from '$lib/components/ui/button';
	import * as Tooltip from '$lib/components/ui/tooltip';
	import { app } from '$lib/state/app.svelte';
	import { cn } from '$lib/utils';

	interface Props {
		class?: string;
		size?: 'default' | 'sm';
	}

	let { class: className, size = 'default' }: Props = $props();

	const uid = $props.id();

	/** Why a new conversation cannot start, or null when it can. */
	const blockedReason = $derived.by((): string | null => {
		if (!app.hasProviders) {
			return 'Aucun fournisseur configuré.';
		}
		const chats = app.chatsProject;
		if (!chats) return 'Conversations indisponibles.';
		if (chats.missing) return 'Le dossier des conversations est introuvable.';
		return null;
	});

	let starting = $state(false);

	async function start() {
		if (blockedReason || starting) return;
		starting = true;
		try {
			await app.newConversation(app.chatsProject?.id);
		} finally {
			starting = false;
		}
	}
</script>

<!-- aria-disabled rather than disabled: the reason stays reachable by hover and by screen readers. -->
<Tooltip.Root>
	<Tooltip.Trigger>
		{#snippet child({ props })}
			<Button
				{...props}
				variant="ghost"
				{size}
				aria-disabled={blockedReason ? 'true' : undefined}
				aria-describedby={blockedReason ? uid : undefined}
				class={cn('aria-disabled:opacity-50', className)}
				onclick={start}
			>
				<SquarePen class="size-4" aria-hidden="true" />
				Nouvelle conversation
			</Button>
		{/snippet}
	</Tooltip.Trigger>
	{#if blockedReason}
		<Tooltip.Content side="right">{blockedReason}</Tooltip.Content>
	{/if}
</Tooltip.Root>
{#if blockedReason}
	<span id={uid} class="sr-only">{blockedReason}</span>
{/if}
