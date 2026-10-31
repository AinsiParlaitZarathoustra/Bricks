/**
 * Theme state for Bricks.
 *
 * - `theme.mode` is the user's choice: 'light' | 'dark' | 'system'.
 * - `theme.resolved` is what is actually shown: 'light' | 'dark'.
 * - The `dark` class is toggled on <html>; CSS tokens are generated from src/lib/theme/tokens.json.
 *
 * Persistence is the caller's job (prefs IPC): read the saved value, pass it
 * to `initTheme()` once at startup, and call `setTheme()` when the user picks
 * a mode, then save it. Until then, the theme last shown (`last-theme.ts`, a
 * display copy) is used, so a dark setup does not start light.
 *
 * Custom colours come from `app.prefs.colors` for the resolved
 * mode. A module-level effect applies them as soon as preferences load, when
 * the mode changes and when the user edits a colour, so no caller has to
 * remember to apply them at start-up.
 *
 * `fontsReady` resolves once the bundled Google Sans faces are loaded (the terminal
 * refits after it).
 */

import { app } from '$lib/state/app.svelte';
import { applyPaletteVariables, paletteVariables } from './palette';
import tokens from './tokens.json';
import { readLastTheme, rememberTheme } from './last-theme';

export type ThemeMode = 'light' | 'dark' | 'system';
export type ResolvedTheme = 'light' | 'dark';

const DARK_QUERY = '(prefers-color-scheme: dark)';

export function isThemeMode(value: unknown): value is ThemeMode {
	return value === 'light' || value === 'dark' || value === 'system';
}

function systemPrefersDark(): boolean {
	return typeof window !== 'undefined' && typeof window.matchMedia === 'function'
		? window.matchMedia(DARK_QUERY).matches
		: false;
}

class ThemeState {
	#mode = $state<ThemeMode>(readLastTheme() ?? 'system');
	#systemDark = $state(systemPrefersDark());
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
	rememberTheme(resolved);
}

// The theme last shown, from the first paint on (the preferences load later).
applyToDocument(state.resolved);

// Custom colours of the resolved mode. Runs in its own root: it lives as long as the app.
if (typeof document !== 'undefined') {
	$effect.root(() => {
		$effect(() => {
			applyPaletteVariables(document.documentElement, paletteVariables(app.prefs.colors?.[state.resolved]));
		});
	});
}

/** Resolves once the bundled Google Sans faces (UI and mono) are loaded. Never rejects: the fallback fonts stay if a face fails. */
export const fontsReady: Promise<void> = loadBundledFonts();

async function loadBundledFonts(): Promise<void> {
	if (typeof document === 'undefined' || !document.fonts) return;
	try {
		await Promise.all([
			document.fonts.load(`${tokens.fonts.ui.defaultWeight} 1em "${tokens.fonts.ui.family}"`),
			document.fonts.load(`500 1em "${tokens.fonts.ui.family}"`),
			document.fonts.load(`400 1em "${tokens.fonts.code.family}"`)
		]);
		await document.fonts.ready;
	} catch {
		// A face that fails to load leaves the system fallback in place.
	}
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
