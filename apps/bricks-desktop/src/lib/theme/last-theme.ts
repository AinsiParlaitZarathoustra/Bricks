/**
 * The theme last shown, kept in this window's storage: the start screen and the
 * first paint use it before the preferences (prefs IPC) are loaded. It is a copy
 * for display only; the preference itself stays in the prefs file.
 */
import type { ResolvedTheme } from './theme.svelte';

const KEY = 'bricks.lastTheme';

/** The theme last shown, or `null` when none was kept (first start, storage unavailable). */
export function readLastTheme(): ResolvedTheme | null {
	try {
		const value = globalThis.localStorage?.getItem(KEY);
		return value === 'light' || value === 'dark' ? value : null;
	} catch {
		return null;
	}
}

export function rememberTheme(resolved: ResolvedTheme): void {
	try {
		globalThis.localStorage?.setItem(KEY, resolved);
	} catch {
		// Storage unavailable: the next start follows the system.
	}
}
