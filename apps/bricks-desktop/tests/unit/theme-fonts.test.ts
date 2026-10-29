// Local font packaging and CSS wiring for the interface and code views.
import { existsSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import tokens from '../../src/lib/theme/tokens.json';

const appRoot = fileURLToPath(new URL('../..', import.meta.url));
const css = readFileSync(`${appRoot}/src/styles/fonts.css`, 'utf8');
const generated = readFileSync(`${appRoot}/src/styles/tokens.generated.css`, 'utf8');
const base = readFileSync(`${appRoot}/src/styles/base.css`, 'utf8');

describe('bundled Google Sans fonts', () => {
	it('declares the interface and both code styles as local variable faces', () => {
		const faces = css.match(/@font-face\s*\{[^}]*\}/g) ?? [];
		expect(faces).toHaveLength(3);
		const ui = faces.find((face) => face.includes('GoogleSansFlex-Variable.woff2'));
		const code = faces.find((face) => face.includes('GoogleSansCode-Variable.woff2'));
		const italic = faces.find((face) => face.includes('GoogleSansCode-Italic-Variable.woff2'));
		expect(ui).toContain("font-family: 'Google Sans Flex';");
		expect(ui).toContain('font-weight: 1 1000;');
		expect(code).toContain("font-family: 'Google Sans Code';");
		expect(code).toContain('font-style: normal;');
		expect(italic).toContain('font-style: italic;');
		for (const face of faces) {
			expect(face).toContain('font-display: swap;');
			expect(face).not.toMatch(/https?:\/\//);
		}
	});

	it('uses Google Sans Flex 400 for body text and Google Sans Code for code', () => {
		expect(tokens.fonts.ui.defaultWeight).toBe(400);
		expect(generated).toContain("--ui-font-family: 'Google Sans Flex',");
		expect(generated).toContain("--code-font-family: 'Google Sans Code',");
		expect(generated).toContain('--ui-font-weight: 400;');
		expect(base).toContain('font-weight: var(--ui-font-weight);');
		expect(base).toContain('font-family: var(--code-font-family);');
	});

	it('ships valid WOFF2 assets and their licences for both families', () => {
		for (const font of [tokens.fonts.ui, tokens.fonts.code]) {
			const directory = `${appRoot}/src/assets/fonts/${font.directory}`;
			expect(readFileSync(`${directory}/OFL.txt`, 'utf8')).toContain('SIL OPEN FONT LICENSE');
			for (const face of font.faces) {
				const bytes = readFileSync(`${directory}/${face.file}`);
				expect(bytes.subarray(0, 4).toString()).toBe('wOF2');
			}
		}
	});

	it('does not retain Geist declarations or assets', () => {
		expect(css + generated).not.toMatch(/Geist/i);
		expect(existsSync(`${appRoot}/src/assets/fonts/geist`)).toBe(false);
	});
});
