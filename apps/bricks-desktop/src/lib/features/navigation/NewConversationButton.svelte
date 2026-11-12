<script lang="ts">
	import { t } from '$lib/i18n/i18n.svelte';
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
			return t('common.noProviderConfigured');
		}
		const chats = app.chatsProject;
		if (!chats) return t('navigation.conversationsUnavailable');
		if (chats.missing) return t('navigation.theConversationsFolderCantBeFound');
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
				<SquarePen aria-hidden="true" data-icon="inline-start" />
				{t('common.newConversation')}
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
