/**
 * Layout breakpoints, sizes and limits shared by the app shell.
 * All widths are CSS pixels of the window (or of a container, where stated).
 */

/** At or above this window width: left, centre and right columns are inline. */
export const WIDE_BREAKPOINT = 1200;

/**
 * Below this window width: both side panels become temporary Sheets.
 * Between NARROW_BREAKPOINT and WIDE_BREAKPOINT: the right panel becomes a
 * temporary Sheet and the left panel stays inline.
 */
export const NARROW_BREAKPOINT = 800;

/** Starting sizes used when the caller provides no saved preference. */
export const LAYOUT_DEFAULTS = {
	leftWidth: 260,
	rightWidth: 280
} as const;

/**
 * Terminal pane share of the centre column (Sprint 14.3). The saved ratio is kept
 * within min..max; `fallback` is used until a ratio is known (no preference loaded yet).
 */
export const TERMINAL_RATIO = {
	min: 0.1,
	max: 0.8,
	fallback: 0.3
} as const;

/** Hard limits. The centre column never shrinks below `centerMin` while sides are inline. */
export const LAYOUT_LIMITS = {
	sideMin: 200,
	sideMax: 480,
	centerMin: 480,
	terminalMin: 120,
	conversationMin: 240
} as const;

export type LayoutMode = 'wide' | 'medium' | 'narrow';

export function layoutModeFor(windowWidth: number): LayoutMode {
	if (windowWidth >= WIDE_BREAKPOINT) return 'wide';
	if (windowWidth >= NARROW_BREAKPOINT) return 'medium';
	return 'narrow';
}
