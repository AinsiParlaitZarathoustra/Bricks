import type { Component } from 'svelte';

/** Heading level a component renders its title with. Pick the level that fits the page outline. */
export type HeadingLevel = 2 | 3 | 4 | 5 | 6;

/** A lucide icon component (or any component taking class and aria-hidden). */
export type IconComponent = Component<{ class?: string; 'aria-hidden'?: boolean | 'true' | 'false' }>;

/** Layout state that the caller may persist (prefs IPC). Emitted by AppShell after user changes. */
export interface ShellLayout {
	leftOpen: boolean;
	rightOpen: boolean;
	terminalOpen: boolean;
	/** Preferred inline width of the left panel, px. */
	leftWidth: number;
	/** Preferred inline width of the right panel, px. */
	rightWidth: number;
	/** Preferred height of the terminal pane, px. */
	terminalHeight: number;
}

/**
 * Toggle state and callbacks handed to the topbar. A callback that is
 * undefined means the corresponding pane is not provided, so its toggle is hidden.
 * Spread it onto <TopBar {...controls} />.
 */
export interface ShellControls {
	leftPressed: boolean;
	rightPressed: boolean;
	terminalPressed: boolean;
	onToggleLeft?: () => void;
	onToggleRight?: () => void;
	onToggleTerminal?: () => void;
}
