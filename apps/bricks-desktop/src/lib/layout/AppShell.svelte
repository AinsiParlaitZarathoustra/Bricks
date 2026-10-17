<script lang="ts">
	import type { Snippet } from 'svelte';
	import { tick, untrack } from 'svelte';
	import { innerWidth } from 'svelte/reactivity/window';
	import { PaneGroup, Pane } from 'paneforge';
	import * as Resizable from '$lib/components/ui/resizable/index.js';
	import * as Sheet from '$lib/components/ui/sheet/index.js';
	import TopBar from './TopBar.svelte';
	import {
		LAYOUT_DEFAULTS,
		LAYOUT_LIMITS,
		TERMINAL_RATIO,
		WIDE_BREAKPOINT,
		layoutModeFor
	} from './constants.js';
	import {
		clampRatio,
		fitSidebars,
		fitTerminalRatio,
		migrateTerminalRatio,
		toPercent,
		toPx
	} from './geometry.js';
	import type { ShellControls, ShellLayout } from './types.js';

	interface Props {
		/** Left navigation column (inline at >= 800px, Sheet below). */
		left?: Snippet;
		/** Conversation column. Always mounted, so its state survives panel toggles. */
		center: Snippet;
		/** Right context column (inline at >= 1200px, Sheet below). */
		right?: Snippet;
		/** Bottom terminal pane inside the centre column. Mounted only while open. */
		terminal?: Snippet;
		/** Top bar. Receives the toggle controls; defaults to a plain TopBar with those toggles. */
		topbar?: Snippet<[ShellControls]>;
		/** User preferences. Automatic collapses never change these. */
		leftOpen?: boolean;
		rightOpen?: boolean;
		terminalOpen?: boolean;
		/** Preferred inline widths, in px. */
		leftWidth?: number;
		rightWidth?: number;
		/**
		 * Terminal share of the centre column, 0.1–0.8. `null` while no ratio is saved:
		 * it is then derived once from `legacyTerminalHeight` (the saved pixel height).
		 */
		terminalRatio?: number | null;
		/** Saved terminal height in px (legacy preference). Undefined until preferences are loaded. */
		legacyTerminalHeight?: number;
		/** Called after the user resizes or toggles a pane (debounced ~300 ms). Not called for automatic changes. */
		onLayoutChange?: (layout: ShellLayout) => void;
	}

	let {
		left,
		center,
		right,
		terminal,
		topbar,
		leftOpen = $bindable(true),
		rightOpen = $bindable(true),
		terminalOpen = $bindable(false),
		leftWidth = $bindable(LAYOUT_DEFAULTS.leftWidth),
		rightWidth = $bindable(LAYOUT_DEFAULTS.rightWidth),
		terminalRatio = $bindable(null),
		legacyTerminalHeight,
		onLayoutChange
	}: Props = $props();

	/** The part of paneforge's group API that AppShell uses. */
	interface GroupApi {
		getLayout: () => number[];
		setLayout: (layout: number[]) => void;
	}

	// Measured sizes of the horizontal body and the centre column, in px.
	let bodyWidth = $state(0);
	let centerHeight = $state(0);
	let hGroup = $state<GroupApi | undefined>(undefined);
	let vGroup = $state<GroupApi | undefined>(undefined);

	// Breakpoints. Viewport width decides the layout mode; it never writes preferences.
	const viewportWidth = $derived(innerWidth.current ?? WIDE_BREAKPOINT);
	const mode = $derived(layoutModeFor(viewportWidth));
	const leftIsOverlay = $derived(mode === 'narrow');
	const rightIsOverlay = $derived(mode !== 'wide');

	// Temporary Sheets opened by the user in overlay modes. Closed on every breakpoint change.
	let leftOverlay = $state(false);
	let rightOverlay = $state(false);
	$effect(() => {
		if (mode) untrack(() => {
			leftOverlay = false;
			rightOverlay = false;
		});
	});

	// Inline visibility: preference AND not forced into an overlay.
	const leftInline = $derived(Boolean(left) && leftOpen && !leftIsOverlay);
	const rightInline = $derived(Boolean(right) && rightOpen && !rightIsOverlay);
	const terminalInline = $derived(Boolean(terminal) && terminalOpen);

	// Horizontal geometry (px -> paneforge percentages).
	const sides = $derived(
		fitSidebars(bodyWidth, leftInline ? leftWidth : null, rightInline ? rightWidth : null)
	);
	const leftPct = $derived(sides.left === null ? 0 : toPercent(sides.left, bodyWidth));
	const rightPct = $derived(sides.right === null ? 0 : toPercent(sides.right, bodyWidth));
	const centerPct = $derived(100 - leftPct - rightPct);
	const sideMinPct = $derived(toPercent(LAYOUT_LIMITS.sideMin, bodyWidth));
	const centerMinPct = $derived(toPercent(LAYOUT_LIMITS.centerMin, bodyWidth));
	const maxSidePx = (other: number | null) =>
		Math.min(
			LAYOUT_LIMITS.sideMax,
			Math.max(LAYOUT_LIMITS.sideMin, bodyWidth - LAYOUT_LIMITS.centerMin - (other ?? 0))
		);
	const leftMaxPct = $derived(toPercent(maxSidePx(sides.right), bodyWidth));
	const rightMaxPct = $derived(toPercent(maxSidePx(sides.left), bodyWidth));

	// Vertical geometry inside the centre column. The terminal keeps its share of the
	// column, clamped to the pixel limits of both panes.
	const termPx = $derived(
		fitTerminalRatio(centerHeight, terminalRatio ?? TERMINAL_RATIO.fallback)
	);
	const termPct = $derived(toPercent(termPx, centerHeight));
	const convPct = $derived(100 - termPct);
	const termMinPct = $derived(toPercent(LAYOUT_LIMITS.terminalMin, centerHeight));
	const termMaxPct = $derived(
		toPercent(
			Math.max(LAYOUT_LIMITS.terminalMin, centerHeight - LAYOUT_LIMITS.conversationMin),
			centerHeight
		)
	);
	const convMinPct = $derived(toPercent(LAYOUT_LIMITS.conversationMin, centerHeight));

	function horizontalTargets(): number[] {
		const out: number[] = [];
		if (leftInline) out.push(leftPct);
		out.push(centerPct);
		if (rightInline) out.push(rightPct);
		return out;
	}

	function verticalTargets(): number[] {
		return terminalInline ? [convPct, termPct] : [100];
	}

	/** Pushes target percentages into paneforge only when they differ, which avoids feedback loops. */
	function applyLayout(group: GroupApi | undefined, targets: number[]): void {
		if (!group || targets.length === 0) return;
		const current = group.getLayout();
		const same =
			current.length === targets.length &&
			current.every((value, i) => Math.abs(value - targets[i]) < 0.1);
		if (!same) group.setLayout(targets);
	}

	$effect(() => {
		const targets = horizontalTargets();
		const group = hGroup;
		if (!group || bodyWidth <= 0) return;
		let cancelled = false;
		// Wait for paneforge to register panes that were just mounted or removed.
		tick().then(() => {
			if (!cancelled) applyLayout(hGroup, targets);
		});
		return () => {
			cancelled = true;
		};
	});

	$effect(() => {
		const targets = verticalTargets();
		const group = vGroup;
		if (!group || centerHeight <= 0) return;
		let cancelled = false;
		tick().then(() => {
			if (!cancelled) applyLayout(vGroup, targets);
		});
		return () => {
			cancelled = true;
		};
	});

	// User-driven resizing. Only these changes write the px preferences.
	let draggingDepth = 0;
	let focusBeforeDrag: HTMLElement | null = null;

	function isUserResizing(): boolean {
		if (draggingDepth > 0) return true;
		const active = document.activeElement;
		return active instanceof HTMLElement && active.hasAttribute('data-pane-resizer');
	}

	function onHandleDragging(dragging: boolean): void {
		if (dragging) {
			if (draggingDepth === 0) {
				// Mousedown moves focus onto the handle. Remember the previous owner (e.g. the composer).
				const active = document.activeElement;
				focusBeforeDrag =
					active instanceof HTMLElement && !active.hasAttribute('data-pane-resizer') ? active : null;
			}
			draggingDepth += 1;
			return;
		}
		draggingDepth = Math.max(0, draggingDepth - 1);
		if (draggingDepth === 0) {
			const previous = focusBeforeDrag;
			focusBeforeDrag = null;
			if (previous && previous.isConnected) previous.focus({ preventScroll: true });
			scheduleEmit();
		}
	}

	function onHorizontalLayout(layout: number[]): void {
		if (bodyWidth <= 0 || !isUserResizing()) return;
		let i = 0;
		if (leftInline) leftWidth = toPx(layout[i++] ?? 0, bodyWidth);
		i++; // centre
		if (rightInline) rightWidth = toPx(layout[i++] ?? 0, bodyWidth);
		scheduleEmit();
	}

	function onVerticalLayout(layout: number[]): void {
		if (!terminalInline || centerHeight <= 0 || !isUserResizing()) return;
		terminalRatio = clampRatio((layout[1] ?? 0) / 100);
		scheduleEmit();
	}

	// Debounced change notification for the caller (persistence).
	let emitTimer: ReturnType<typeof setTimeout> | undefined;
	let lastEmitted = JSON.stringify(snapshot());

	function snapshot(): ShellLayout {
		return {
			leftOpen,
			rightOpen,
			terminalOpen,
			leftWidth,
			rightWidth,
			terminalRatio: clampRatio(terminalRatio ?? TERMINAL_RATIO.fallback)
		};
	}

	function scheduleEmit(): void {
		clearTimeout(emitTimer);
		emitTimer = setTimeout(() => {
			const current = snapshot();
			const key = JSON.stringify(current);
			if (key === lastEmitted) return;
			lastEmitted = key;
			onLayoutChange?.(current);
		}, 300);
	}

	$effect(() => () => clearTimeout(emitTimer));

	// One-time migration: a saved pixel height becomes a share of the centre column.
	// The result is saved through the normal layout change, like a user resize.
	$effect(() => {
		const ratio = migrateTerminalRatio(terminalRatio, legacyTerminalHeight, centerHeight);
		if (ratio === null) return;
		untrack(() => {
			terminalRatio = ratio;
			// The snapshot at start-up already showed the fallback ratio: a migrated value
			// equal to it must still be saved.
			lastEmitted = '';
			scheduleEmit();
		});
	});

	function toggleLeft(): void {
		if (leftIsOverlay) leftOverlay = !leftOverlay;
		else leftOpen = !leftOpen;
		scheduleEmit();
	}

	function toggleRight(): void {
		if (rightIsOverlay) rightOverlay = !rightOverlay;
		else rightOpen = !rightOpen;
		scheduleEmit();
	}

	function toggleTerminal(): void {
		terminalOpen = !terminalOpen;
		scheduleEmit();
	}

	const controls: ShellControls = $derived({
		leftPressed: leftIsOverlay ? leftOverlay : leftOpen,
		rightPressed: rightIsOverlay ? rightOverlay : rightOpen,
		terminalPressed: terminalOpen,
		onToggleLeft: left ? toggleLeft : undefined,
		onToggleRight: right ? toggleRight : undefined,
		onToggleTerminal: terminal ? toggleTerminal : undefined
	});
</script>

<div class="flex h-dvh min-h-0 w-full flex-col overflow-hidden bg-background text-foreground">
	<h1 class="sr-only">Bricks</h1>
	{#if topbar}
		{@render topbar(controls)}
	{:else}
		<TopBar {...controls} />
	{/if}

	<div class="min-h-0 flex-1" bind:clientWidth={bodyWidth}>
		<PaneGroup direction="horizontal" bind:this={hGroup} onLayoutChange={onHorizontalLayout} class="flex h-full w-full">
			{#if leftInline && left}
				<Pane id="bricks-left" order={1} defaultSize={leftPct} minSize={sideMinPct} maxSize={leftMaxPct}>
					<div class="flex h-full min-h-0 min-w-0 flex-col overflow-hidden bg-sidebar text-sidebar-foreground">
						{@render left()}
					</div>
				</Pane>
				<Resizable.Handle class="bg-border/50" aria-label="Redimensionner le panneau de gauche" onDraggingChange={onHandleDragging} />
			{/if}

			<Pane id="bricks-center" order={2} defaultSize={centerPct} minSize={centerMinPct}>
				<div class="flex h-full min-w-0 flex-col" bind:clientHeight={centerHeight}>
					<PaneGroup direction="vertical" bind:this={vGroup} onLayoutChange={onVerticalLayout} class="flex h-full w-full flex-col">
						<Pane id="bricks-conversation" order={1} defaultSize={convPct} minSize={convMinPct}>
							<main aria-label="Conversation" class="flex h-full min-h-0 min-w-0 flex-col bg-background">
								{@render center()}
							</main>
						</Pane>
						{#if terminalInline && terminal}
							<Resizable.Handle class="bg-border/50" aria-label="Redimensionner le terminal" onDraggingChange={onHandleDragging} />
							<Pane id="bricks-terminal" order={2} defaultSize={termPct} minSize={termMinPct} maxSize={termMaxPct}>
								<section aria-label="Terminal" class="flex h-full min-h-0 flex-col bg-background">
									{@render terminal()}
								</section>
							</Pane>
						{/if}
					</PaneGroup>
				</div>
			</Pane>

			{#if rightInline && right}
				<Resizable.Handle class="bg-border/50" aria-label="Redimensionner le panneau de droite" onDraggingChange={onHandleDragging} />
				<Pane id="bricks-right" order={3} defaultSize={rightPct} minSize={sideMinPct} maxSize={rightMaxPct}>
					<aside aria-label="Contexte" class="flex h-full min-h-0 min-w-0 flex-col overflow-hidden bg-sidebar text-sidebar-foreground">
						{@render right()}
					</aside>
				</Pane>
			{/if}
		</PaneGroup>
	</div>
</div>

{#if left && leftIsOverlay}
	<Sheet.Root bind:open={leftOverlay}>
		<Sheet.Content side="left" class="gap-0 p-0" style="width: min(20rem, 88vw)">
			<Sheet.Title class="sr-only">Panneau de gauche</Sheet.Title>
			<div class="flex h-full min-h-0 flex-col overflow-hidden">
				{@render left()}
			</div>
		</Sheet.Content>
	</Sheet.Root>
{/if}

{#if right && rightIsOverlay}
	<Sheet.Root bind:open={rightOverlay}>
		<Sheet.Content side="right" class="gap-0 p-0 data-[side=right]:sm:max-w-none" style="width: min({rightWidth}px, 92vw)">
			<Sheet.Title class="sr-only">Panneau de droite</Sheet.Title>
			<aside aria-label="Contexte" class="flex h-full min-h-0 flex-col overflow-hidden">
				{@render right()}
			</aside>
		</Sheet.Content>
	</Sheet.Root>
{/if}
