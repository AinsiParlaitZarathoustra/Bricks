<script lang="ts">
	import { t } from '$lib/i18n/i18n.svelte';
	import ChevronLeftIcon from '@lucide/svelte/icons/chevron-left';
	import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
	import { Button } from '$lib/components/ui/button';

	interface Props {
		/** Page shown, counted from 0. */
		page: number;
		pages: number;
		disabled?: boolean;
		onchange: (page: number) => void;
	}

	let { page, pages, disabled = false, onchange }: Props = $props();
</script>

{#if pages > 1}
	<nav aria-label={t('catalog.pages')} class="flex items-center justify-between gap-2">
		<Button
			variant="outline"
			size="sm"
			disabled={disabled || page <= 0}
			onclick={() => onchange(page - 1)}
		>
			<ChevronLeftIcon aria-hidden="true" />
			{t('catalog.previous')}
		</Button>
		<span class="text-xs tabular-nums text-muted-foreground">{t('common.pageOf', { value: page + 1, pages })}</span>
		<Button
			variant="outline"
			size="sm"
			disabled={disabled || page >= pages - 1}
			onclick={() => onchange(page + 1)}
		>
			{t('catalog.next')}
			<ChevronRightIcon aria-hidden="true" />
		</Button>
	</nav>
{/if}
