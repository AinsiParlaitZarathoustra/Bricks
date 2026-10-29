import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { generateTokens } from '../../scripts/generate-tokens.mjs';

const appRoot = fileURLToPath(new URL('../..', import.meta.url));
const tailwind = readFileSync(`${appRoot}/src/styles/tailwind.css`, 'utf8');
const generated = readFileSync(`${appRoot}/src/styles/tokens.generated.css`, 'utf8');

describe('generated design tokens', () => {
	it('matches the source JSON without rewriting generated files', () => {
		expect(generateTokens({ check: true })).toEqual([]);
	});

	it('adds the four shared text sizes without assigning line heights', () => {
		for (const [name, value] of [['ui', '13px'], ['caption', '11px'], ['code', '13px'], ['compact', '12.5px']]) {
			expect(generated).toContain(`--${name}-font-size: ${value};`);
			expect(tailwind).toContain(`--text-${name}: var(--${name}-font-size);`);
			expect(tailwind).not.toContain(`--text-${name}--line-height`);
		}
	});
});
