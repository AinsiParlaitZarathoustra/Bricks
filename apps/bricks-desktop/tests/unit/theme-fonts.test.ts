// Regression checks on the bundled fonts: Geist for the interface and Geist Mono for code and the
// terminal, both served from local files (no network at launch), and no Inter left.
import { existsSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const appRoot = fileURLToPath(new URL('../..', import.meta.url));
const css = readFileSync(`${appRoot}/src/app.css`, 'utf8');

describe('bundled fonts', () => {
	it('declares Geist and Geist Mono as local, variable, swapped faces', () => {
		const faces = css.match(/@font-face\s*\{[^}]*\}/g) ?? [];
		expect(faces).toHaveLength(3);
		const roman = faces.find((f) => f.includes('Geist-Variable.woff2'));
		const italic = faces.find((f) => f.includes('Geist-Italic-Variable.woff2'));
		const mono = faces.find((f) => f.includes('GeistMono-Variable.woff2'));
		expect(roman).toMatch(/font-family:\s*'Geist';/);
		expect(roman).toMatch(/font-style:\s*normal;/);
		expect(italic).toMatch(/font-family:\s*'Geist';/);
		expect(italic).toMatch(/font-style:\s*italic;/);
		expect(mono).toMatch(/font-family:\s*'Geist Mono';/);
		for (const face of faces) {
			expect(face).toMatch(/font-display:\s*swap;/);
			expect(face).toMatch(/font-weight:\s*100 900;/);
			expect(face).not.toMatch(/https?:\/\//);
		}
	});

	it('uses Geist for the UI and Geist Mono for code', () => {
		expect(css).toMatch(/--font-sans:\s*'Geist',/);
		expect(css).toMatch(/--font-mono:\s*'Geist Mono',/);
	});

	it('no longer imports Inter', () => {
		expect(css).not.toMatch(/fontsource/);
		expect(css).not.toMatch(/Inter/);
	});

	it('ships the font files and their licence in the repository', () => {
		for (const file of ['Geist-Variable.woff2', 'Geist-Italic-Variable.woff2', 'GeistMono-Variable.woff2', 'OFL.txt']) {
			expect(existsSync(`${appRoot}/src/assets/fonts/geist/${file}`)).toBe(true);
		}
	});
});
