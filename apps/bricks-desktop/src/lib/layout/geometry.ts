import { LAYOUT_LIMITS } from './constants.js';

/**
 * Pure layout maths for AppShell. Pane sizes are stored in px (user
 * preferences) and converted to percentages for paneforge at render time.
 */

export interface SidebarFit {
	/** Width to display for the left panel in px, or null when it is not inline. */
	left: number | null;
	/** Width to display for the right panel in px, or null when it is not inline. */
	right: number | null;
}

/**
 * Returns the widths to display so that the centre column keeps at least
 * `centerMin` px. Each side is first clamped to [sideMin, sideMax]; if the
 * window is still too small, the sides shrink proportionally to their room
 * above `sideMin`. The caller's preferences are NOT modified by this function.
 */
export function fitSidebars(
	totalWidth: number,
	left: number | null,
	right: number | null
): SidebarFit {
	const { sideMin, sideMax, centerMin } = LAYOUT_LIMITS;
	const clamp = (value: number) => Math.min(sideMax, Math.max(sideMin, value));

	let l = left === null ? null : clamp(left);
	let r = right === null ? null : clamp(right);

	const sides = (l ?? 0) + (r ?? 0);
	const excess = sides + centerMin - totalWidth;
	if (excess > 0 && sides > 0) {
		const roomL = l === null ? 0 : l - sideMin;
		const roomR = r === null ? 0 : r - sideMin;
		const room = roomL + roomR;
		if (room > 0) {
			const cut = Math.min(excess, room);
			if (l !== null) l -= cut * (roomL / room);
			if (r !== null) r -= cut * (roomR / room);
		}
	}
	return { left: l, right: r };
}

/** Converts a px size to a paneforge percentage of its container. */
export function toPercent(px: number, containerPx: number): number {
	if (containerPx <= 0) return 0;
	return (px / containerPx) * 100;
}

/** Converts a paneforge percentage back to px, rounded to whole pixels. */
export function toPx(percent: number, containerPx: number): number {
	return Math.round((percent / 100) * containerPx);
}

/**
 * Clamps the terminal height so the conversation pane keeps its minimum.
 * Returns px, never less than `terminalMin`.
 */
export function fitTerminal(containerHeight: number, terminalHeight: number): number {
	const { terminalMin, conversationMin } = LAYOUT_LIMITS;
	const maxHeight = Math.max(terminalMin, containerHeight - conversationMin);
	return Math.min(maxHeight, Math.max(terminalMin, terminalHeight));
}
