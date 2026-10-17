<script lang="ts">
	import type { Snippet } from 'svelte';
	import PanelLeftIcon from '@lucide/svelte/icons/panel-left';
	import PanelRightIcon from '@lucide/svelte/icons/panel-right';
	import PanelBottomIcon from '@lucide/svelte/icons/panel-bottom';
	import GitCompareIcon from '@lucide/svelte/icons/git-compare';
	import PanelToggle from './PanelToggle.svelte';
	import { cn } from '$lib/utils.js';

	interface Props {
		/** Centre slot: the conversation title and its project. Truncated when long. */
		title?: Snippet;
		/** Right slot: actions, rendered before the panel toggles. */
		actions?: Snippet;
		/** Visibility of the left panel. Its toggle sits at the top left, shown only when onToggleLeft is set. */
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

	const hasRightToggles = $derived(Boolean(onToggleRight || onToggleTerminal || onToggleChanges));
</script>

<!--
	Three columns: the side columns are equal whatever the right side holds, so the
	centre (title and project) stays in the middle. The centre is capped at 28 rem or
	half the bar, and truncates. Empty areas are the Tauri drag region; controls are not.
-->
<header
	data-tauri-drag-region
	class={cn(
		'grid h-11 shrink-0 grid-cols-[minmax(0,1fr)_minmax(0,min(28rem,50%))_minmax(0,1fr)] items-center gap-2 border-b border-border bg-background px-3',
		className
	)}
>
	<div data-tauri-drag-region class="flex min-w-0 items-center justify-self-start">
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
	</div>

	<div data-tauri-drag-region class="flex min-w-0 items-center justify-center">
		{@render title?.()}
	</div>

	<div data-tauri-drag-region class="flex min-w-0 items-center justify-self-end gap-1">
		{#if actions}
			<div class="flex shrink-0 items-center gap-1">
				{@render actions()}
			</div>
		{/if}

		{#if hasRightToggles}
			<div role="group" aria-label="Affichage" class="ml-1 flex shrink-0 items-center gap-0.5 border-l border-border pl-2">
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
	</div>
</header>
