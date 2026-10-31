// Static checks of the two « Maintenir l’ordinateur actif » controls and of their wiring (sources
// read, not rendered). The menu Plus sets the open conversation only, with the sub-text « Uniquement
// pour cette session » and no full stop. Paramètres › Général is one switch on the saved preference,
// with its title alone. App.svelte asks the host to keep awake from both. Same approach as
// context-card-structure.test.ts.
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const src = (path: string) => readFileSync(new URL(`../../src/${path}`, import.meta.url), 'utf8');

const PLUS = 'lib/features/context/PlusMenu.svelte';
const GENERAL = 'lib/features/settings/GeneralSettings.svelte';
const APP = 'App.svelte';

describe('menu Plus: the item is for this conversation only', () => {
	const plus = src(PLUS);

	it('is a checkbox item that stays open when chosen', () => {
		expect(plus).toContain('<DropdownMenuPrimitive.CheckboxItem');
		expect(plus).toContain('closeOnSelect={false}');
	});

	it('reads and sets the open conversation, never the saved preference', () => {
		expect(plus).toContain('const keepAwake = $derived(app.current?.keepAwake ?? false);');
		expect(plus).toContain('if (app.current) app.current.keepAwake = on;');
		expect(plus).not.toContain('setPrefs');
		expect(plus).not.toContain('keep_awake');
	});

	it('is titled « Maintenir l’ordinateur actif », with the sub-text « Uniquement pour cette session » and no full stop', () => {
		expect(plus).toContain('<span class="text-sm">Maintenir l’ordinateur actif</span>');
		expect(plus).toContain('<span class="text-xs text-muted-foreground">Uniquement pour cette session</span>');
		expect(plus).not.toContain('Uniquement pour cette session.');
	});
});

describe('Paramètres › Général: one switch, no sub-text', () => {
	const general = src(GENERAL);

	it('is one switch on the saved preference', () => {
		expect(general).toContain(
			'<Switch checked={app.prefs.keep_awake} onCheckedChange={(on) => app.setPrefs({ keep_awake: on })} />'
		);
	});

	it('its label is the title alone, the switch inside it', () => {
		expect(general).toMatch(/<label class="[^"]*">\s*Maintenir l’ordinateur actif\s*<Switch/);
	});

	it('has no sub-text: nothing about the session, and no description line', () => {
		expect(general).not.toContain('Uniquement pour cette session');
		expect(general).not.toMatch(/Maintenir l’ordinateur actif\s*<\/span>\s*<span/);
	});
});

describe('App: the host keeps awake from either wish', () => {
	it('asks for the preference or a running conversation that asked for it', () => {
		const app = src(APP);
		expect(app).toContain("import { keepAwakeWanted } from '$lib/state/keepAwake'");
		expect(app).toContain('keepAwakeWanted(app.prefs.keep_awake, app.conversations.values())');
		expect(app).toContain('keepAwakeSet(wanted)');
	});
});
