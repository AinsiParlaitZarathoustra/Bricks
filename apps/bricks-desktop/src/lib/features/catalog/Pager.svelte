<script lang="ts">
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
	<nav aria-label="Pages" class="flex items-center justify-between gap-2">
		<Button
			variant="outline"
			size="sm"
			disabled={disabled || page <= 0}
			onclick={() => onchange(page - 1)}
		>
			<ChevronLeftIcon aria-hidden="true" />
			Précédente
		</Button>
		<span class="text-xs tabular-nums text-muted-foreground">Page {page + 1} sur {pages}</span>
		<Button
			variant="outline"
			size="sm"
			disabled={disabled || page >= pages - 1}
			onclick={() => onchange(page + 1)}
		>
			Suivante
			<ChevronRightIcon aria-hidden="true" />
		</Button>
	</nav>
{/if}
