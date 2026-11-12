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
	import MoveLeftIcon from '@lucide/svelte/icons/move-left';
	import MoveRightIcon from '@lucide/svelte/icons/move-right';
	import { Button } from '$lib/components/ui/button';
	import * as Tooltip from '$lib/components/ui/tooltip';
	import { Separator } from '$lib/components/ui/separator';
	import PanelToggle from './PanelToggle.svelte';
	import { TOP_BAR } from './constants.js';
	import { cn } from '$lib/utils.js';

	interface Props {
		/** Where you are: the conversation and its project, left-aligned after the navigation. Truncated when long. */
		title?: Snippet;
		/** After the history arrows: the search of conversations. */
		search?: Snippet;
		/** History of the conversations visited. The arrows show only when onBack is set. */
		onBack?: () => void;
		onForward?: () => void;
		canGoBack?: boolean;
		canGoForward?: boolean;
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
		search,
		onBack,
		onForward,
		canGoBack = false,
		canGoForward = false,
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

{#snippet arrow(label: string, Icon: typeof MoveLeftIcon, enabled: boolean, onclick?: () => void)}
	<Tooltip.Root>
		<Tooltip.Trigger>
			{#snippet child({ props })}
				<Button {...props} variant="ghost" size="icon-sm" class="text-muted-foreground" aria-label={label} disabled={!enabled} {onclick}>
					<Icon aria-hidden="true" />
				</Button>
			{/snippet}
		</Tooltip.Trigger>
		<Tooltip.Content side="bottom" sideOffset={6}>{label}</Tooltip.Content>
	</Tooltip.Root>
{/snippet}

<!--
	Like the bars of desktop apps: on the left the panel toggle, the history arrows and the search,
	then where you are (left-aligned, truncated); on the right the view toggles and « Plus ».
	Empty areas are the Tauri drag region; controls are not.
-->
<header
	data-tauri-drag-region
	class={cn('flex shrink-0 items-center gap-2 bg-sidebar px-3', className)}
	style:height="{TOP_BAR.height}px"
	style:padding-left={systemButtons ? `${TOP_BAR.macSystemButtons}px` : undefined}
>
	<div data-tauri-drag-region class="flex shrink-0 items-center gap-0.5">
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
		{#if onBack}
			<Tooltip.Provider delayDuration={400}>
				{@render arrow(t('common.previous'), MoveLeftIcon, canGoBack, onBack)}
				{@render arrow(t('common.next'), MoveRightIcon, canGoForward, onForward)}
				{#if search}
					{@render search()}
				{/if}
			</Tooltip.Provider>
		{/if}
	</div>

	<div data-tauri-drag-region class="ml-3 flex min-w-0 flex-1 items-center">
		{@render title?.()}
	</div>

	<div data-tauri-drag-region class="flex shrink-0 items-center gap-1">
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
					<Separator orientation="vertical" decorative class="mx-1 h-5 self-center" />
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
