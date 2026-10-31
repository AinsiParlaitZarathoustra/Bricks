// The reload keys of the built app (src/lib/layout/reload.ts): F5, Cmd+R and Ctrl+R, with or
// without Shift, are the keys the window refuses. Alt combinations, a bare R and the other keys
// pass. Pure functions: the keyboard event is a plain object, no DOM is needed.
import { describe, expect, it, vi } from 'vitest';
import { blockReload, isReloadShortcut } from '../../src/lib/layout/reload';

type Key = Pick<KeyboardEvent, 'key' | 'metaKey' | 'ctrlKey' | 'altKey'>;

function press(overrides: Partial<Key> & Pick<Key, 'key'>): Key {
	return { metaKey: false, ctrlKey: false, altKey: false, ...overrides };
}

/** A keydown as the window handler receives it, with a spy on preventDefault. */
function keydown(overrides: Partial<Key> & Pick<Key, 'key'>) {
	const preventDefault = vi.fn();
	const event = { ...press(overrides), preventDefault } as unknown as KeyboardEvent;
	return { event, preventDefault };
}

describe('isReloadShortcut: the reload keys', () => {
	it('F5 reloads, alone or with a modifier', () => {
		expect(isReloadShortcut(press({ key: 'F5' }))).toBe(true);
		expect(isReloadShortcut(press({ key: 'F5', metaKey: true }))).toBe(true);
		expect(isReloadShortcut(press({ key: 'F5', ctrlKey: true }))).toBe(true);
	});

	it('Cmd+R and Ctrl+R reload', () => {
		expect(isReloadShortcut(press({ key: 'r', metaKey: true }))).toBe(true);
		expect(isReloadShortcut(press({ key: 'r', ctrlKey: true }))).toBe(true);
	});

	it('Cmd+Shift+R and Ctrl+Shift+R reload: with Shift the key is the capital letter', () => {
		expect(isReloadShortcut(press({ key: 'R', metaKey: true }))).toBe(true);
		expect(isReloadShortcut(press({ key: 'R', ctrlKey: true }))).toBe(true);
	});

	it('Alt+R with Cmd or Ctrl does not reload', () => {
		expect(isReloadShortcut(press({ key: 'r', metaKey: true, altKey: true }))).toBe(false);
		expect(isReloadShortcut(press({ key: 'r', ctrlKey: true, altKey: true }))).toBe(false);
		expect(isReloadShortcut(press({ key: 'R', metaKey: true, altKey: true }))).toBe(false);
	});

	it('a bare R, or Alt+R alone, does not reload', () => {
		expect(isReloadShortcut(press({ key: 'r' }))).toBe(false);
		expect(isReloadShortcut(press({ key: 'R' }))).toBe(false);
		expect(isReloadShortcut(press({ key: 'r', altKey: true }))).toBe(false);
	});

	it('other shortcuts with the same modifiers do not reload', () => {
		expect(isReloadShortcut(press({ key: 's', metaKey: true }))).toBe(false);
		expect(isReloadShortcut(press({ key: 'q', metaKey: true }))).toBe(false);
		expect(isReloadShortcut(press({ key: 'f', ctrlKey: true }))).toBe(false);
		expect(isReloadShortcut(press({ key: 'F4', metaKey: true }))).toBe(false);
	});
});

describe('blockReload: the window handler', () => {
	it('prevents the default of F5 and of Cmd+R, Ctrl+R and their Shift forms', () => {
		for (const overrides of [
			{ key: 'F5' },
			{ key: 'r', metaKey: true },
			{ key: 'r', ctrlKey: true },
			{ key: 'R', metaKey: true },
			{ key: 'R', ctrlKey: true }
		]) {
			const { event, preventDefault } = keydown(overrides);
			blockReload(event);
			expect(preventDefault, JSON.stringify(overrides)).toHaveBeenCalledTimes(1);
		}
	});

	it('leaves the other keys alone: no preventDefault for Alt+R, a bare R or Cmd+S', () => {
		for (const overrides of [
			{ key: 'r', metaKey: true, altKey: true },
			{ key: 'r' },
			{ key: 'R' },
			{ key: 's', metaKey: true }
		]) {
			const { event, preventDefault } = keydown(overrides);
			blockReload(event);
			expect(preventDefault, JSON.stringify(overrides)).not.toHaveBeenCalled();
		}
	});
});
