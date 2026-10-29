// Unit tests for the colour palette of the theme: hex and HSV conversions, WCAG contrast, the theme
// tokens in hex, and the CSS variables that a custom colour sets (and only those).
import { describe, expect, it } from 'vitest';
import {
	PALETTE_VARIABLES,
	TEXT_DARK,
	TEXT_LIGHT,
	TOKENS,
	applyPaletteVariables,
	contrastRatio,
	hexToHsv,
	hsvToHex,
	mixHex,
	mutedTextOn,
	normalizeHex,
	oklchToHex,
	paletteVariables,
	textOn
} from '../../src/lib/theme/palette';

describe('normalizeHex', () => {
	it('accepts six or three digits, with or without #, in lower case', () => {
		expect(normalizeHex('#A4512C')).toBe('#a4512c');
		expect(normalizeHex(' a4512c ')).toBe('#a4512c');
		expect(normalizeHex('#fa0')).toBe('#ffaa00');
	});

	it('refuses anything that is not a colour', () => {
		expect(normalizeHex('#a4512')).toBeNull();
		expect(normalizeHex('rouge')).toBeNull();
		expect(normalizeHex('#gggggg')).toBeNull();
		expect(normalizeHex('')).toBeNull();
	});
});

describe('HSV conversions', () => {
	it('round-trips a colour through HSV', () => {
		for (const hex of ['#a4512c', '#2f5fa8', '#14171c', '#fbfaf7', '#808080']) {
			expect(hsvToHex(hexToHsv(hex))).toBe(hex);
		}
	});

	it('reads pure colours at their hue', () => {
		expect(hexToHsv('#ff0000')).toEqual({ h: 0, s: 1, v: 1 });
		expect(hexToHsv('#00ff00').h).toBeCloseTo(120);
		expect(hexToHsv('#0000ff').h).toBeCloseTo(240);
	});

	it('gives a grey no saturation', () => {
		expect(hexToHsv('#808080').s).toBe(0);
	});
});

describe('contrast', () => {
	it('is 21 between black and white, and 1 between a colour and itself', () => {
		expect(contrastRatio('#000000', '#ffffff')).toBeCloseTo(21, 5);
		expect(contrastRatio('#a4512c', '#a4512c')).toBeCloseTo(1, 5);
	});

	it('picks the more readable of the two text colours', () => {
		expect(textOn('#ffffff')).toBe(TEXT_DARK);
		expect(textOn('#14171c')).toBe(TEXT_LIGHT);
		expect(contrastRatio(textOn('#a4512c'), '#a4512c')).toBeGreaterThan(3);
	});

	it('keeps secondary text at 4.5:1 when a mix can, and stays between text and background', () => {
		const background = '#fbfaf7';
		const muted = mutedTextOn(background, TEXT_DARK);
		expect(contrastRatio(muted, background)).toBeGreaterThanOrEqual(4.5);
		expect(contrastRatio(muted, background)).toBeLessThan(contrastRatio(TEXT_DARK, background));
	});

	it('mixes in sRGB at the given weight', () => {
		expect(mixHex('#000000', '#ffffff', 0)).toBe('#000000');
		expect(mixHex('#000000', '#ffffff', 1)).toBe('#ffffff');
		expect(mixHex('#000000', '#ffffff', 0.5)).toBe('#808080');
	});
});

describe('theme tokens in hex', () => {
	it('converts OKLCH tokens of src/app.css', () => {
		expect(oklchToHex(1, 0, 0)).toBe('#ffffff');
		expect(oklchToHex(0, 0, 0)).toBe('#000000');
	});

	it('the default accent is the same blue in both modes: #002aff', () => {
		expect(oklchToHex(0.4755, 0.2973, 264.2)).toBe('#002aff');
		expect(TOKENS.light.accent).toBe('#002aff');
		expect(TOKENS.dark.accent).toBe('#002aff');
	});

	it('keeps the light and dark defaults readable as they are', () => {
		expect(contrastRatio(TOKENS.light.foreground, TOKENS.light.background)).toBeGreaterThan(12);
		expect(contrastRatio(TOKENS.dark.foreground, TOKENS.dark.background)).toBeGreaterThan(10);
		expect(contrastRatio(TOKENS.light.mutedForeground, TOKENS.light.background)).toBeGreaterThan(4.5);
	});
});

describe('paletteVariables', () => {
	it('sets nothing when no colour is customised (the theme tokens apply)', () => {
		expect(paletteVariables(undefined)).toEqual({});
		expect(paletteVariables({ accent: null, background: null, panel: null })).toEqual({});
	});

	it('maps the accent to the primary, ring and sidebar primary, with a readable foreground', () => {
		const vars = paletteVariables({ accent: '#2f5fa8', background: null, panel: null });
		expect(Object.keys(vars).sort()).toEqual(
			['--primary', '--primary-foreground', '--ring', '--sidebar-primary', '--sidebar-primary-foreground', '--sidebar-ring'].sort()
		);
		expect(vars['--primary']).toBe('#2f5fa8');
		expect(vars['--ring']).toBe('#2f5fa8');
		expect(contrastRatio(vars['--primary-foreground'], '#2f5fa8')).toBeGreaterThanOrEqual(4.5);
	});

	it('maps the central background to the background, with readable text colours', () => {
		const vars = paletteVariables({ accent: null, background: '#14171c', panel: null });
		expect(vars['--background']).toBe('#14171c');
		expect(vars['--foreground']).toBe(TEXT_LIGHT);
		expect(contrastRatio(vars['--muted-foreground'], '#14171c')).toBeGreaterThanOrEqual(4.5);
		expect(vars).not.toHaveProperty('--sidebar');
	});

	it('maps the panels to the sidebar surface, with readable text', () => {
		const vars = paletteVariables({ accent: null, background: null, panel: '#262a31' });
		expect(vars['--sidebar']).toBe('#262a31');
		expect(vars['--sidebar-foreground']).toBe(TEXT_LIGHT);
		expect(vars).not.toHaveProperty('--background');
	});

	it('never sets a status, error or diff colour', () => {
		const full = paletteVariables({ accent: '#2f5fa8', background: '#fbfaf7', panel: '#e6e9ee' });
		for (const name of Object.keys(full)) {
			expect(name).not.toMatch(/success|warning|info|destructive|diff|status|code/);
		}
	});

	it('only uses variables that the apply step clears on reset', () => {
		const full = paletteVariables({ accent: '#2f5fa8', background: '#fbfaf7', panel: '#e6e9ee' });
		for (const name of Object.keys(full)) {
			expect(PALETTE_VARIABLES).toContain(name);
		}
	});

	it('ignores a value that is not a colour', () => {
		expect(paletteVariables({ accent: 'bleu', background: null, panel: null })).toEqual({});
	});
});

describe('applyPaletteVariables', () => {
	function fakeRoot() {
		const values = new Map<string, string>();
		const style = {
			setProperty: (name: string, value: string) => values.set(name, value),
			removeProperty: (name: string) => values.delete(name)
		};
		return { style, values } as unknown as HTMLElement & { values: Map<string, string> };
	}

	it('sets the given variables and removes the others, so a reset restores the theme', () => {
		const root = fakeRoot();
		applyPaletteVariables(root, { '--primary': '#2f5fa8', '--ring': '#2f5fa8' });
		expect(root.values.get('--primary')).toBe('#2f5fa8');

		applyPaletteVariables(root, { '--background': '#14171c' });
		expect(root.values.has('--primary')).toBe(false);
		expect(root.values.get('--background')).toBe('#14171c');
	});
});
