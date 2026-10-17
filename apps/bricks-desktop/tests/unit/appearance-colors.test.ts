// Unit tests for the appearance choices: what a role shows (custom value or theme token), the
// foreground of its preview, the contrast warnings, and the presets offered.
import { describe, expect, it } from 'vitest';
import {
	COLOR_ROLES,
	COLOR_ROLE_LABELS,
	PRESETS,
	TEXT_DARK,
	TEXT_LIGHT,
	TOKENS,
	colorChecks,
	contrastRatio,
	isCustomRole,
	normalizeHex,
	roleColor,
	roleForeground
} from '../../src/lib/theme/palette';

describe('role labels', () => {
	it('names the three choices in French, in order', () => {
		expect(COLOR_ROLES).toEqual(['accent', 'background', 'panel']);
		expect(COLOR_ROLES.map((role) => COLOR_ROLE_LABELS[role])).toEqual(['Accent', 'Fond principal', 'Fond des panneaux']);
	});
});

describe('roleColor and isCustomRole', () => {
	it('shows the theme token for a mode without a custom value', () => {
		expect(roleColor(undefined, 'light', 'accent')).toBe(TOKENS.light.accent);
		expect(roleColor({ accent: null, background: null, panel: null }, 'dark', 'panel')).toBe(TOKENS.dark.panel);
		expect(isCustomRole({ accent: null, background: null, panel: null }, 'accent')).toBe(false);
	});

	it('shows the custom value once it is set, and only for that role', () => {
		const colors = { accent: '#2f5fa8', background: null, panel: null };
		expect(roleColor(colors, 'light', 'accent')).toBe('#2f5fa8');
		expect(roleColor(colors, 'light', 'background')).toBe(TOKENS.light.background);
		expect(isCustomRole(colors, 'accent')).toBe(true);
		expect(isCustomRole(colors, 'panel')).toBe(false);
	});

	it('treats an invalid stored value as no value', () => {
		expect(isCustomRole({ accent: 'pas-une-couleur', background: null, panel: null }, 'accent')).toBe(false);
	});
});

describe('roleForeground', () => {
	it('uses the theme foreground for an uncustomised surface', () => {
		expect(roleForeground(undefined, 'light', 'background')).toBe(TOKENS.light.foreground);
		expect(roleForeground(undefined, 'dark', 'panel')).toBe(TOKENS.dark.panelForeground);
	});

	it('picks a readable text colour on a customised surface', () => {
		expect(roleForeground({ accent: null, background: '#14171c', panel: null }, 'light', 'background')).toBe(TEXT_LIGHT);
		expect(roleForeground({ accent: null, background: '#fbfaf7', panel: null }, 'dark', 'background')).toBe(TEXT_DARK);
	});
});

describe('colorChecks', () => {
	it('checks the focus ring (3:1) and the accent text (4.5:1) for the accent', () => {
		const checks = colorChecks({ accent: '#2f5fa8', background: null, panel: null }, 'light', 'accent');
		expect(checks.map((c) => c.min)).toEqual([3, 4.5]);
		expect(checks[1].ratio).toBeGreaterThanOrEqual(4.5);
	});

	it('reports a weak focus colour on the background as below its minimum, without blocking the choice', () => {
		const checks = colorChecks({ accent: '#fbf8f2', background: '#fbfaf7', panel: null }, 'light', 'accent');
		expect(checks[0].ratio).toBeLessThan(checks[0].min);
	});

	it('checks the text of the central background and of the panels against 4.5:1', () => {
		const onBackground = colorChecks({ accent: null, background: '#1c1b19', panel: null }, 'dark', 'background');
		expect(onBackground).toHaveLength(1);
		expect(onBackground[0].min).toBe(4.5);
		expect(onBackground[0].ratio).toBeGreaterThanOrEqual(4.5);

		const onPanel = colorChecks({ accent: null, background: null, panel: '#262a31' }, 'dark', 'panel');
		expect(onPanel[0].ratio).toBeGreaterThanOrEqual(4.5);
	});
});

describe('presets', () => {
	it('offers valid, named colours for each role', () => {
		for (const role of COLOR_ROLES) {
			expect(PRESETS[role].length).toBeGreaterThan(3);
			for (const swatch of PRESETS[role]) {
				expect(normalizeHex(swatch.hex)).toBe(swatch.hex);
				expect(swatch.label.trim()).not.toBe('');
			}
		}
	});

	it('keeps the accent presets distinct from each other', () => {
		const hexes = PRESETS.accent.map((s) => s.hex);
		expect(new Set(hexes).size).toBe(hexes.length);
		expect(contrastRatio(PRESETS.accent[0].hex, '#ffffff')).toBeGreaterThan(3);
	});
});
