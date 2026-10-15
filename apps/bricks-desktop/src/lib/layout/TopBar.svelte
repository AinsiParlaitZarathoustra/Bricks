<script lang="ts">
	import type { Snippet } from 'svelte';
	import PanelLeftIcon from '@lucide/svelte/icons/panel-left';
	import PanelRightIcon from '@lucide/svelte/icons/panel-right';
	import PanelBottomIcon from '@lucide/svelte/icons/panel-bottom';
	import GitCompareIcon from '@lucide/svelte/icons/git-compare';
	import PanelToggle from './PanelToggle.svelte';
	import { cn } from '$lib/utils.js';

	interface Props {
		/** Left slot: title area (session name, breadcrumb...). */
		title?: Snippet;
		/** Right slot: actions, rendered before the panel toggles. */
		actions?: Snippet;
		/** Visibility of the left panel. Toggle shown only when onToggleLeft is provided. */
		leftPressed?: boolean;
		onToggleLeft?: () => void;
		rightPressed?: boolean;
		onToggleRight?: () => void;
		terminalPressed?: boolean;
		onToggleTerminal?: () => void;
		/** Visibility of the changes view (independent of the right panel). Toggle shown only when onToggleChanges is provided. */
		changesPressed?: boolean;
		onToggleChanges?: () => void;
		class?: string;
	}

	let {
		title,
		actions,
		leftPressed = false,
		onToggleLeft,
		rightPressed = false,
		onToggleRight,
		terminalPressed = false,
		onToggleTerminal,
		changesPressed = false,
		onToggleChanges,
		class: className
	}: Props = $props();

	const hasToggles = $derived(
		Boolean(onToggleLeft || onToggleRight || onToggleTerminal || onToggleChanges)
	);
</script>

<header
	data-tauri-drag-region
	class={cn('flex h-11 shrink-0 items-center gap-2 border-b border-border bg-background px-3', className)}
>
	<div class="flex min-w-0 flex-1 items-center gap-2">
		{@render title?.()}
	</div>

	{#if actions}
		<div class="flex shrink-0 items-center gap-1">
			{@render actions()}
		</div>
	{/if}

	{#if hasToggles}
		<div role="group" aria-label="Affichage" class="ml-1 flex shrink-0 items-center gap-0.5 border-l border-border pl-2">
			{#if onToggleLeft}
				<PanelToggle
					ariaLabel="Panneau de gauche"
					tooltipShow="Afficher le panneau de gauche"
					tooltipHide="Masquer le panneau de gauche"
					pressed={leftPressed}
					icon={PanelLeftIcon}
					onclick={onToggleLeft}
				/>
			{/if}
			{#if onToggleRight}
				<PanelToggle
					ariaLabel="Panneau de droite"
					tooltipShow="Afficher le panneau de droite"
					tooltipHide="Masquer le panneau de droite"
					pressed={rightPressed}
					icon={PanelRightIcon}
					onclick={onToggleRight}
				/>
			{/if}
			{#if onToggleTerminal}
				<PanelToggle
					ariaLabel="Terminal"
					tooltipShow="Afficher le terminal"
					tooltipHide="Masquer le terminal"
					pressed={terminalPressed}
					icon={PanelBottomIcon}
					onclick={onToggleTerminal}
				/>
			{/if}
			{#if onToggleChanges}
				<PanelToggle
					ariaLabel="Modifications"
					tooltipShow="Afficher les modifications"
					tooltipHide="Masquer les modifications"
					pressed={changesPressed}
					icon={GitCompareIcon}
					onclick={onToggleChanges}
				/>
			{/if}
		</div>
	{/if}
</header>
