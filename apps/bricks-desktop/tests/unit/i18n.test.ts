// @vitest-environment happy-dom
// The interface language: the two catalogues (same keys, same placeholders, same forms), the
// plural forms and the local formats, the switch of language (text, page lang, saved choice),
// the reactivity of a mounted component, and the version label. The setup file has set French;
// the default (English) is checked last, on a fresh module.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import { en, type Message } from '../../src/lib/i18n/messages/en';
import { fr } from '../../src/lib/i18n/messages/fr';
import * as i18n from '../../src/lib/i18n/i18n.svelte';
import { versionLabel } from '../../src/lib/features/settings/format';
import SvgViewer from '../../src/lib/features/conversation/markdown/SvgViewer.svelte';

// The sanitiser is replaced by the identity: this file checks the labels, not the cleaning.
vi.mock('../../src/lib/features/conversation/markdown/sanitize', async (importOriginal) => ({
	...(await importOriginal<typeof import('../../src/lib/features/conversation/markdown/sanitize')>()),
	sanitizeSvg: (svg: string) => svg
}));

const { setLanguage, t } = i18n;

/** The strings of a message: the text, or its two plural forms. */
function forms(message: Message): Record<string, string> {
	return typeof message === 'string' ? { text: message } : { one: message.one, other: message.other };
}

/** The `{placeholders}` of a text, sorted. */
function placeholders(text: string): string[] {
	return [...text.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort();
}

describe('the two catalogues', () => {
	it('have exactly the same keys', () => {
		expect(Object.keys(fr).sort()).toEqual(Object.keys(en).sort());
	});

	it('have the same placeholders in each pair of texts', () => {
		for (const key of Object.keys(en) as (keyof typeof en)[]) {
			const english = forms(en[key]);
			const french = forms(fr[key]);
			expect(Object.keys(french), key).toEqual(Object.keys(english));
			for (const form of Object.keys(english)) {
				expect(placeholders(french[form]), `${key} (${form})`).toEqual(placeholders(english[form]));
			}
		}
	});

	it('have the same shape: a text in both, or plural forms (one and other) in both', () => {
		for (const key of Object.keys(en) as (keyof typeof en)[]) {
			expect(typeof fr[key], key).toBe(typeof en[key]);
			if (typeof en[key] !== 'string') expect(Object.keys(fr[key] as object).sort(), key).toEqual(['one', 'other']);
		}
	});

	it('have no empty value', () => {
		for (const [name, catalogue] of [['en', en], ['fr', fr]] as const) {
			for (const [key, message] of Object.entries(catalogue as Record<string, Message>)) {
				for (const [form, text] of Object.entries(forms(message))) {
					expect(text.trim(), `${name} ${key} (${form})`).not.toBe('');
				}
			}
		}
	});
});

describe('switching the language', () => {
	beforeEach(() => setLanguage('fr'));

	it('changes t() at once, the lang of the page and the saved choice', () => {
		setLanguage('en');
		expect(t('common.noValue')).toBe('none');
		expect(document.documentElement.lang).toBe('en');
		expect(localStorage.getItem('bricks.language')).toBe('en');

		setLanguage('fr');
		expect(t('common.noValue')).toBe('aucun');
		expect(document.documentElement.lang).toBe('fr');
		expect(localStorage.getItem('bricks.language')).toBe('fr');
	});

	it('keeps the same figures and the same plural rule per language', () => {
		setLanguage('en');
		expect(t('settings.noteCount', { count: 0 })).toBe('0 notes');
		expect(t('settings.noteCount', { count: 1 })).toBe('1 note');
		expect(t('settings.noteCount', { count: 2 })).toBe('2 notes');
		expect(t('settings.noteCount', { count: 1000 })).toBe('1,000 notes');

		setLanguage('fr');
		// French takes the singular for 0, and groups thousands with a narrow no-break space.
		expect(t('settings.noteCount', { count: 0 })).toBe('0 note');
		expect(t('settings.noteCount', { count: 1 })).toBe('1 note');
		expect(t('settings.noteCount', { count: 2 })).toBe('2 notes');
		expect(t('settings.noteCount', { count: 1000 })).toBe('1 000 notes');
	});
});

describe('local formats follow the language', () => {
	beforeEach(() => setLanguage('fr'));

	it('numberFormat, declared once, uses the language of the moment', () => {
		const format = i18n.numberFormat({ maximumFractionDigits: 1 });
		setLanguage('en');
		expect(format.format(1234.5)).toBe('1,234.5');
		setLanguage('fr');
		expect(format.format(1234.5)).toBe('1 234,5');
	});

	it('dateTimeFormat, declared once, names the month in the language of the moment', () => {
		const format = i18n.dateTimeFormat({ month: 'long', timeZone: 'UTC' });
		const january = new Date(Date.UTC(2026, 0, 15));
		setLanguage('en');
		expect(format.format(january)).toBe('January');
		setLanguage('fr');
		expect(format.format(january)).toBe('janvier');
	});

	it('relativeTimeFormat, declared once, speaks the language of the moment', () => {
		const format = i18n.relativeTimeFormat({ numeric: 'auto' });
		setLanguage('en');
		expect(format.format(-1, 'day')).toBe('yesterday');
		setLanguage('fr');
		expect(format.format(-1, 'day')).toBe('hier');
	});
});

describe('a mounted component follows a change of language', () => {
	const DRAWING =
		'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10" width="10" height="10"><rect width="10" height="10"/></svg>';
	let target: HTMLDivElement;
	let app: ReturnType<typeof mount> | null = null;

	beforeEach(() => {
		setLanguage('fr');
		target = document.createElement('div');
		document.body.appendChild(target);
		app = mount(SvgViewer, { target, props: { svg: DRAWING, title: 'Dessin', fileName: 'dessin.svg', variant: 'drawing' } });
		flushSync();
	});

	/** The drawing is prepared asynchronously: its toolbar appears a few turns after the mount. */
	async function settle() {
		for (let i = 0; i < 5; i++) {
			await new Promise((resolve) => setTimeout(resolve, 0));
			flushSync();
		}
	}

	afterEach(() => {
		if (app) void unmount(app);
		app = null;
		target.remove();
		setLanguage('fr');
	});

	it('shows its buttons in French, then in English once the language is switched', async () => {
		await settle();
		expect(target.querySelector('button[aria-label="Exporter en PNG"]')).not.toBeNull();
		expect(target.querySelector('button[aria-label="Export as PNG"]')).toBeNull();

		setLanguage('en');
		await settle();

		expect(target.querySelector('button[aria-label="Export as PNG"]')).not.toBeNull();
		expect(target.querySelector('button[aria-label="Exporter en PNG"]')).toBeNull();
	});
});

describe('the version label', () => {
	it('names a pre-release by its stage, and a release by its minor version', () => {
		expect(versionLabel('1.0.0-alpha.1')).toBe('Bricks 1.0 Alpha');
		expect(versionLabel('1.2.3')).toBe('Bricks 1.2');
	});
});

describe('without a saved choice, the interface starts in English', () => {
	it('a fresh module, with no saved language, shows English', async () => {
		vi.resetModules();
		localStorage.clear();
		const fresh = await import('../../src/lib/i18n/i18n.svelte');
		expect(fresh.currentLanguage()).toBe('en');
		expect(fresh.t('common.noValue')).toBe('none');
		expect(document.documentElement.lang).toBe('en');
	});
});
