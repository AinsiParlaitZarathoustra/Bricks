/**
 * Theme state for Bricks.
 *
 * - `theme.mode` is the user's choice: 'light' | 'dark' | 'system'.
 * - `theme.resolved` is what is actually shown: 'light' | 'dark'.
 * - The `dark` class is toggled on <html>; CSS tokens live in src/app.css.
 *
 * Persistence is the caller's job (prefs IPC): read the saved value, pass it
 * to `initTheme()` once at startup, and call `setTheme()` when the user picks
 * a mode, then save it. This module never touches storage.
 */

export type ThemeMode = 'light' | 'dark' | 'system';
export type ResolvedTheme = 'light' | 'dark';

const DARK_QUERY = '(prefers-color-scheme: dark)';

export function isThemeMode(value: unknown): value is ThemeMode {
	return value === 'light' || value === 'dark' || value === 'system';
}

class ThemeState {
	#mode = $state<ThemeMode>('system');
	#systemDark = $state(false);
	#resolved = $derived<ResolvedTheme>(
		this.#mode === 'system' ? (this.#systemDark ? 'dark' : 'light') : this.#mode
	);

	get mode(): ThemeMode {
		return this.#mode;
	}

	get resolved(): ResolvedTheme {
		return this.#resolved;
	}

	set mode(value: ThemeMode) {
		this.#mode = value;
	}

	set systemDark(value: boolean) {
		this.#systemDark = value;
	}
}

const state = new ThemeState();

/** Reactive read-only view of the theme. Read `theme.mode` or `theme.resolved` in components. */
export const theme = {
	get mode(): ThemeMode {
		return state.mode;
	},
	get resolved(): ResolvedTheme {
		return state.resolved;
	}
};

let mediaQuery: MediaQueryList | null = null;
let disposeListener: (() => void) | null = null;

function applyToDocument(resolved: ResolvedTheme): void {
	if (typeof document === 'undefined') return;
	document.documentElement.classList.toggle('dark', resolved === 'dark');
}

/** Sets the user's theme mode and updates <html> immediately. Does not persist. */
export function setTheme(mode: ThemeMode): void {
	state.mode = mode;
	applyToDocument(state.resolved);
}

/**
 * Starts following the operating system colour scheme and applies the initial
 * mode. Call once at startup with the saved preference (default 'system').
 * Returns a disposer that stops listening to system changes.
 */
export function initTheme(initial: ThemeMode = 'system'): () => void {
	disposeListener?.();
	disposeListener = null;

	if (typeof window !== 'undefined' && typeof window.matchMedia === 'function') {
		mediaQuery = window.matchMedia(DARK_QUERY);
		state.systemDark = mediaQuery.matches;

		const onChange = (event: MediaQueryListEvent) => {
			state.systemDark = event.matches;
			applyToDocument(state.resolved);
		};
		mediaQuery.addEventListener('change', onChange);
		disposeListener = () => mediaQuery?.removeEventListener('change', onChange);
	}

	setTheme(initial);
	return () => {
		disposeListener?.();
		disposeListener = null;
	};
}
