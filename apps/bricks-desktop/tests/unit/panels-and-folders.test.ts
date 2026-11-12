// The side panels share the main area's background in both themes, the dark
// theme sits on a deep black, a folder lists three conversations before
// « Afficher plus », and a folder without Git says so without a final period.
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import tokens from '../../src/lib/theme/tokens.json';
import { bound, call } from './french-keys';

const source = (path: string) => readFileSync(new URL(`../../src/${path}`, import.meta.url), 'utf8');

describe('theme: a frame around one inset surface', () => {
	it.each(['light', 'dark'] as const)('the %s frame (top bar and left panel) is darker than the surface', (mode) => {
		const [frame] = tokens.colors[mode].sidebar as number[];
		const [surface] = tokens.colors[mode].background as number[];
		expect(surface - frame).toBeGreaterThanOrEqual(0.015);
		expect(surface - frame).toBeLessThanOrEqual(0.04);
	});

	it('the dark background is a deep black under a light text', () => {
		const [background] = tokens.colors.dark.background as number[];
		const [foreground] = tokens.colors.dark.foreground as number[];
		expect(background).toBeLessThanOrEqual(0.15);
		expect(foreground - background).toBeGreaterThanOrEqual(0.8);
	});
});

describe('folders and Git', () => {
	it('lists three conversations per folder before « Afficher plus »', () => {
		expect(source('lib/features/navigation/ProjectList.svelte')).toContain('const SHOWN_PER_PROJECT = 3;');
	});

	it('says Git is not installed in the folder, without a final period', () => {
		const env = source('lib/features/context/ProjectEnvironment.svelte');
		expect(env).toContain(`{${call('Git n’est pas installé dans ce dossier')}}</span>`);
		expect(env).not.toContain('n’est pas un dépôt Git');
		expect(source('lib/features/changes/GitChanges.svelte')).toContain(
			bound('title', 'Git n’est pas installé dans ce dossier')
		);
	});
});
