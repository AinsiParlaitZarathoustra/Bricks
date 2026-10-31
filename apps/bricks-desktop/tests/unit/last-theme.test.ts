// The theme last shown (theme/last-theme.ts), kept for the start screen and the first paint: it
// reads back only light or dark, never throws when the storage refuses, and the theme state starts
// on it (theme/theme.svelte.ts). Node environment: the storage is stubbed on globalThis.
import { afterEach, describe, expect, it, vi } from 'vitest';
import { readLastTheme, rememberTheme } from '../../src/lib/theme/last-theme';

/** A storage that keeps its values in a map, like window.localStorage. */
function memoryStorage(initial: Record<string, string> = {}) {
	const data = new Map(Object.entries(initial));
	return {
		data,
		getItem: vi.fn((key: string) => data.get(key) ?? null),
		setItem: vi.fn((key: string, value: string) => {
			data.set(key, String(value));
		}),
		removeItem: vi.fn((key: string) => {
			data.delete(key);
		}),
		clear: vi.fn(() => data.clear()),
		key: vi.fn(() => null),
		get length() {
			return data.size;
		},
	};
}

afterEach(() => {
	vi.unstubAllGlobals();
	vi.resetModules();
});

describe('readLastTheme: only a light or dark theme is read back', () => {
	it('is null when no theme was kept yet', () => {
		vi.stubGlobal('localStorage', memoryStorage());
		expect(readLastTheme()).toBeNull();
	});

	it('gives back the kept theme, light or dark', () => {
		vi.stubGlobal('localStorage', memoryStorage({ 'bricks.lastTheme': 'dark' }));
		expect(readLastTheme()).toBe('dark');
		vi.stubGlobal('localStorage', memoryStorage({ 'bricks.lastTheme': 'light' }));
		expect(readLastTheme()).toBe('light');
	});

	it('ignores any other value, so the system theme applies', () => {
		for (const value of ['system', 'blue', '', 'DARK']) {
			vi.stubGlobal('localStorage', memoryStorage({ 'bricks.lastTheme': value }));
			expect(readLastTheme()).toBeNull();
		}
	});

	it('reads the key bricks.lastTheme and no other', () => {
		const storage = memoryStorage({ 'bricks.theme': 'dark' });
		vi.stubGlobal('localStorage', storage);
		expect(readLastTheme()).toBeNull();
		expect(storage.getItem).toHaveBeenCalledWith('bricks.lastTheme');
	});

	it('is null, not an error, when the storage refuses to be read', () => {
		vi.stubGlobal('localStorage', {
			getItem: () => {
				throw new DOMException('denied', 'SecurityError');
			},
		});
		expect(readLastTheme()).toBeNull();
	});

	it('is null when there is no storage at all', () => {
		vi.stubGlobal('localStorage', undefined);
		expect(readLastTheme()).toBeNull();
	});
});

describe('rememberTheme: the theme shown is kept, and a refusal is not an error', () => {
	it('keeps the theme under bricks.lastTheme, to read it back at the next start', () => {
		const storage = memoryStorage();
		vi.stubGlobal('localStorage', storage);
		rememberTheme('dark');
		expect(storage.setItem).toHaveBeenCalledWith('bricks.lastTheme', 'dark');
		expect(readLastTheme()).toBe('dark');
	});

	it('does not throw when the storage refuses the write', () => {
		vi.stubGlobal('localStorage', {
			setItem: () => {
				throw new Error('quota');
			},
		});
		expect(() => rememberTheme('light')).not.toThrow();
	});

	it('does nothing when there is no storage', () => {
		vi.stubGlobal('localStorage', undefined);
		expect(() => rememberTheme('dark')).not.toThrow();
	});
});

describe('the theme state starts on the theme last shown', () => {
	it('starts on the kept theme, in dark mode as well', async () => {
		vi.stubGlobal('localStorage', memoryStorage({ 'bricks.lastTheme': 'dark' }));
		const { theme } = await import('../../src/lib/theme/theme.svelte');
		expect(theme.mode).toBe('dark');
		expect(theme.resolved).toBe('dark');
	});

	it('starts on the system theme when nothing was kept', async () => {
		vi.stubGlobal('localStorage', memoryStorage());
		const { theme } = await import('../../src/lib/theme/theme.svelte');
		expect(theme.mode).toBe('system');
	});
});
