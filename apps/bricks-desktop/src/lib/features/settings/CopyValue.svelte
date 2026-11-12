<script lang="ts">
	import { t } from '$lib/i18n/i18n.svelte';
	import { onDestroy } from 'svelte';
	import { toast } from 'svelte-sonner';
	import { Button } from '$lib/components/ui/button';
	import Copy from '@lucide/svelte/icons/copy';
	import Check from '@lucide/svelte/icons/check';

	interface Props {
		/** Visible name of the value, used in the button label. */
		label: string;
		value: string;
	}

	let { label, value }: Props = $props();

	let copied = $state(false);
	let resetTimer: ReturnType<typeof setTimeout> | null = null;

	async function copy() {
		try {
			await navigator.clipboard.writeText(value);
			copied = true;
			if (resetTimer) clearTimeout(resetTimer);
			resetTimer = setTimeout(() => (copied = false), 1500);
			toast.success(t('settings.copied', { label }));
		} catch {
			toast.error(t('settings.couldNotCopy', { label }));
		}
	}

	onDestroy(() => {
		if (resetTimer) clearTimeout(resetTimer);
	});
</script>

<div class="flex min-w-0 items-center gap-2">
	<code class="min-w-0 flex-1 truncate rounded-md px-0 py-1 font-mono text-xs text-foreground" title={value}>
		{value}
	</code>
	<Button variant="outline" size="sm" onclick={copy} aria-label={t('settings.copy', { label })}>
		{#if copied}
			<Check aria-hidden="true" data-icon="inline-start" />
		{:else}
			<Copy aria-hidden="true" data-icon="inline-start" />
		{/if}
		{copied ? t('common.copied') : t('common.copy')}
	</Button>
</div>
