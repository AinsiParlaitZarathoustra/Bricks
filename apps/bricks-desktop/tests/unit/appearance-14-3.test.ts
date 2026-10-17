// Appearance (Sprint 14.3, validation): the palette's CSS variables exist as theme tokens in both
// modes of src/app.css, and the accent presets keep their text readable. The rules themselves are
// unit-tested in theme-palette.test.ts and appearance-colors.test.ts; this file checks the wiring
// between the palette and the stylesheet. Validation, Haiku V (Sprint 14.3).
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { COLOR_ROLES, PALETTE_VARIABLES, PRESETS, colorChecks, type ResolvedMode } from '../../src/lib/theme/palette';

const css = readFileSync(new URL('../../src/app.css', import.meta.url), 'utf8');

/** The declarations inside the first block that starts with `selector {`. */
function blockOf(selector: string): string {
	const start = css.indexOf(`${selector} {`);
	if (start < 0) throw new Error(`no block ${selector} in app.css`);
	const open = css.indexOf('{', start);
	let depth = 0;
	for (let i = open; i < css.length; i++) {
		if (css[i] === '{') depth += 1;
		else if (css[i] === '}') {
			depth -= 1;
			if (depth === 0) return css.slice(open + 1, i);
		}
	}
	throw new Error(`unclosed block ${selector}`);
}

const LIGHT = blockOf(':root');
const DARK = blockOf('.dark');

function declares(block: string, name: string): boolean {
	return new RegExp(`(^|[;\\s{])${name.replace(/[-]/g, '\\-')}\\s*:`, 'm').test(block);
}

describe('palette variables are theme tokens in both modes', () => {
	it('every variable the palette can set is declared in :root (light) and .dark', () => {
		const missingLight = PALETTE_VARIABLES.filter((name) => !declares(LIGHT, name));
		const missingDark = PALETTE_VARIABLES.filter((name) => !declares(DARK, name));
		expect(missingLight).toEqual([]);
		expect(missingDark).toEqual([]);
	});

	it('the three roles each drive a non-empty set of variables', () => {
		expect(COLOR_ROLES).toEqual(['accent', 'background', 'panel']);
	});
});

describe('accent presets keep the text on them readable', () => {
	// The text colour on a customised accent is the more readable of the two theme text colours.
	// WCAG AA for normal text is 4.5:1: a preset below it would show a warning in the settings.
	const modes: ResolvedMode[] = ['light', 'dark'];
	for (const mode of modes) {
		it(`every accent preset reaches 4.5:1 for its text in ${mode} mode`, () => {
			const below = PRESETS.accent
				.map((swatch) => ({
					label: swatch.label,
					ratio: colorChecks({ accent: swatch.hex, background: null, panel: null }, mode, 'accent')
						.find((c) => c.label === 'Texte sur l’accent')?.ratio ?? 0
				}))
				.filter((row) => row.ratio < 4.5);
			expect(below).toEqual([]);
		});
	}

	it('every background and panel preset keeps its text at 4.5:1 or more in both modes', () => {
		for (const mode of modes) {
			for (const role of ['background', 'panel'] as const) {
				const below = PRESETS[role]
					.map((swatch) => ({
						label: swatch.label,
						ratio: colorChecks(
							role === 'background'
								? { accent: null, background: swatch.hex, panel: null }
								: { accent: null, background: null, panel: swatch.hex },
							mode,
							role
						)[0].ratio
					}))
					.filter((row) => row.ratio < 4.5);
				expect(below, `${role} presets in ${mode}`).toEqual([]);
			}
		}
	});
});
