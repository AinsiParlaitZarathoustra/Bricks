<script lang="ts">
	import { t } from '$lib/i18n/i18n.svelte';
	import type { Snippet } from 'svelte';
	import PanelLeftOpenIcon from '@lucide/svelte/icons/panel-left-open';
	import PanelLeftCloseIcon from '@lucide/svelte/icons/panel-left-close';
	import PanelRightOpenIcon from '@lucide/svelte/icons/panel-right-open';
	import PanelRightCloseIcon from '@lucide/svelte/icons/panel-right-close';
	import TerminalIcon from '@lucide/svelte/icons/terminal';
	import GitCompareIcon from '@lucide/svelte/icons/git-compare';
	import BookOpenIcon from '@lucide/svelte/icons/book-open';
	import { Separator } from '$lib/components/ui/separator';
	import PanelToggle from './PanelToggle.svelte';
	import { TOP_BAR } from './constants.js';
	import { cn } from '$lib/utils.js';

	interface Props {
		/** Centre slot: the conversation title and its project. Truncated when long. */
		title?: Snippet;
		/** Right slot: actions, rendered before the panel toggles. */
		actions?: Snippet;
		/** Last slot, after the panel toggles (the « Plus » menu). */
		trailing?: Snippet;
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
		/** Visibility of the reader view of the right column. Toggle shown only when onToggleReader is provided. */
		readerPressed?: boolean;
		onToggleReader?: () => void;
		/** Leaves room on the left for the macOS window buttons drawn over the bar. */
		systemButtons?: boolean;
		class?: string;
	}

	let {
		title,
		actions,
		trailing,
		leftPressed = false,
		onToggleLeft,
		rightPressed = false,
		onToggleRight,
		terminalPressed = false,
		onToggleTerminal,
		changesPressed = false,
		onToggleChanges,
		readerPressed = false,
		onToggleReader,
		systemButtons = false,
		class: className
	}: Props = $props();

	const hasRightToggles = $derived(Boolean(onToggleRight || onToggleTerminal || onToggleChanges || onToggleReader));
</script>

<!--
	Three columns: the side columns are equal whatever the right side holds, so the
	centre (title and project) stays in the middle. The centre is capped at 28 rem or
	half the bar, and truncates. Empty areas are the Tauri drag region; controls are not.
-->
<header
	data-tauri-drag-region
	class={cn(
		'grid shrink-0 grid-cols-[minmax(0,1fr)_minmax(0,min(28rem,50%))_minmax(0,1fr)] items-center gap-2 bg-sidebar px-3',
		className
	)}
	style:height="{TOP_BAR.height}px"
	style:padding-left={systemButtons ? `${TOP_BAR.macSystemButtons}px` : undefined}
>
	<div data-tauri-drag-region class="flex min-w-0 items-center justify-self-start">
		{#if onToggleLeft}
			<PanelToggle
				ariaLabel={t('layout.leftPanel')}
				tooltipShow={t('layout.showTheLeftPanel')}
				tooltipHide={t('layout.hideTheLeftPanel')}
				pressed={leftPressed}
				icon={leftPressed ? PanelLeftCloseIcon : PanelLeftOpenIcon}
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
			<div role="group" aria-label={t('layout.view')} class="ml-1 flex shrink-0 items-center gap-0.5">
				{#if onToggleRight}
					<PanelToggle
						ariaLabel={t('layout.rightPanel')}
						tooltipShow={t('layout.showTheRightPanel')}
						tooltipHide={t('layout.hideTheRightPanel')}
						pressed={rightPressed}
						icon={rightPressed ? PanelRightCloseIcon : PanelRightOpenIcon}
						onclick={onToggleRight}
					/>
				{/if}
				{#if onToggleRight && (onToggleTerminal || onToggleChanges || onToggleReader)}
					<Separator orientation="vertical" decorative class="mx-1 h-8 self-center" />
				{/if}
				{#if onToggleTerminal}
					<PanelToggle
						ariaLabel={t('common.terminal')}
						tooltipShow={t('layout.showTheTerminal')}
						tooltipHide={t('common.hideTheTerminal')}
						pressed={terminalPressed}
						icon={TerminalIcon}
						onclick={onToggleTerminal}
					/>
				{/if}
				{#if onToggleChanges}
					<PanelToggle
						ariaLabel={t('common.changes')}
						tooltipShow={t('layout.showChanges')}
						tooltipHide={t('layout.hideChanges')}
						pressed={changesPressed}
						icon={GitCompareIcon}
						onclick={onToggleChanges}
					/>
				{/if}
				{#if onToggleReader}
					<PanelToggle
						ariaLabel={t('common.reader')}
						tooltipShow={t('layout.showTheReader')}
						tooltipHide={t('layout.hideTheReader')}
						pressed={readerPressed}
						icon={BookOpenIcon}
						onclick={onToggleReader}
					/>
				{/if}
			</div>
		{/if}

		{#if trailing}
			<div class="ml-1 flex shrink-0 items-center">
				{@render trailing()}
			</div>
		{/if}
	</div>
</header>
