// The welcome screen of a new conversation: its field of characters (pure maths), its four
// suggestions (they fill the draft, never send, and a replaced draft can be given back), and
// where it is shown.
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import { BAYER, GLYPHS, glyphAt, hollow, noise } from '../../src/lib/features/conversation/welcome/dither';
import { STARTERS, prefillDraft, starterText } from '../../src/lib/features/conversation/welcome/starters';
import WelcomeView from '../../src/lib/features/conversation/welcome/WelcomeView.svelte';
import { setLanguage } from '../../src/lib/i18n/i18n.svelte';

const source = (path: string) => readFileSync(fileURLToPath(new URL(`../../src/${path}`, import.meta.url)), 'utf8');

describe('the field of characters', () => {
	it('keeps the wind within [-1, 1]', () => {
		for (let i = 0; i < 500; i++) {
			const v = noise(i % 37, Math.floor(i / 37), i * 13);
			expect(v).toBeGreaterThanOrEqual(-1);
			expect(v).toBeLessThanOrEqual(1);
		}
	});

	it('leaves the middle empty and fills towards the edges', () => {
		expect(hollow(0)).toBe(0);
		expect(hollow(0.34)).toBe(0);
		expect(hollow(0.84)).toBeCloseTo(1);
		for (let gx = 0; gx < 40; gx++) for (let gy = 0; gy < 40; gy++) expect(glyphAt(gx, gy, 0.2, 7)).toBeNull();
	});

	it('draws only glyphs of the ramp, never a space', () => {
		const seen = new Set<string>();
		for (let gx = 0; gx < 60; gx++)
			for (let gy = 0; gy < 60; gy++) {
				const glyph = glyphAt(gx, gy, 1, gx * 3 + gy);
				if (glyph) seen.add(glyph);
			}
		expect(seen.size).toBeGreaterThan(1);
		for (const glyph of seen) {
			expect(GLYPHS).toContain(glyph);
			expect(glyph).not.toBe(' ');
		}
	});

	it('dithers with a 4 × 4 Bayer matrix holding 0 to 15 once each', () => {
		expect(BAYER.flat().sort((a, b) => a - b)).toEqual([...Array(16).keys()]);
	});
});

describe('the suggestions', () => {
	it('are four, each with a label and a text in English and in French', () => {
		expect(STARTERS).toHaveLength(4);
		for (const language of ['en', 'fr'] as const) {
			setLanguage(language);
			for (const starter of STARTERS) expect(starterText(starter).trim()).not.toBe('');
		}
		setLanguage('fr');
	});

	it('fill an empty draft and report nothing to give back', () => {
		const draft = { text: '' };
		expect(prefillDraft(draft, 'Analyse ce code')).toBeNull();
		expect(draft.text).toBe('Analyse ce code');
	});

	it('give back the draft they replace', () => {
		const draft = { text: 'mon brouillon' };
		expect(prefillDraft(draft, 'Écris des tests pour ')).toBe('mon brouillon');
		expect(draft.text).toBe('Écris des tests pour ');
		expect(prefillDraft({ text: '   ' }, 'x')).toBeNull();
	});
});

describe('the welcome view', () => {
	it('shows the name, the line and four buttons in a named group, and a decorative field', () => {
		setLanguage('fr');
		const html = render(WelcomeView, { props: { onPick: () => {} } }).body;
		expect(html).toContain('Bricks');
		expect(html).toContain('Bienvenue sur Bricks.');
		expect(html).toContain('aria-label="Suggestions"');
		expect(html.match(/<button/g)).toHaveLength(4);
		expect(html).toMatch(/<canvas[^>]*aria-hidden="true"/);
		expect(html).toContain('alt=""');
	});

	it('is shown only for a project with no open conversation, and the provider message stays', () => {
		const app = source('App.svelte');
		expect(app).toMatch(/\{:else if !app\.hasProviders\}[^]*app\.noProviderConfigured[^]*\{:else if app\.currentProject\}\s*<WelcomeView onPick=\{pickStarter\} \/>/);
	});

	it('picking a suggestion fills the composer and focuses it, without sending', () => {
		const app = source('App.svelte');
		const pick = app.slice(app.indexOf('function pickStarter'), app.indexOf('function toggleReader'));
		expect(pick).toContain('prefillDraft(draft, text)');
		expect(pick).toContain('app.composerFocus += 1');
		expect(pick).not.toMatch(/send|newConversation/);
	});
});
