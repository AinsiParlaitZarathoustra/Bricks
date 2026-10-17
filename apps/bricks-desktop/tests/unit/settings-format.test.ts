// Unit tests for the pure helpers of the settings dialog.
import { describe, expect, it } from 'vitest';
import {
	formatCount,
	formatModalities,
	formatTokens,
	platformLabel,
	providersFilePath
} from '../../src/lib/features/settings/format';

describe('settings format helpers', () => {
	it('shows unknown token limits as "Inconnue"', () => {
		expect(formatTokens(null)).toBe('Inconnue');
		expect(formatTokens(undefined)).toBe('Inconnue');
	});

	it('formats token counts with a unit', () => {
		expect(formatTokens(200_000)).toMatch(/^200\D?000 tokens$/);
	});

	it('formats plain counts with digit grouping', () => {
		expect(formatCount(3)).toBe('3');
		expect(formatCount(1234)).toMatch(/^1\D?234$/);
	});

	it('lists input modalities in French and says when none is declared', () => {
		expect(formatModalities(['text', 'image'])).toBe('Texte, Image');
		expect(formatModalities([])).toBe('Non déclarées');
		expect(formatModalities(['hologram'])).toBe('hologram');
	});

	it('names known platforms and keeps unknown ones as they are', () => {
		expect(platformLabel('macos')).toBe('macOS');
		expect(platformLabel('darwin')).toBe('macOS');
		expect(platformLabel('windows')).toBe('Windows');
		expect(platformLabel('freebsd')).toBe('freebsd');
	});

	it('builds the providers file path from the Bricks home, with its separator', () => {
		expect(providersFilePath('/Users/a/.bricks')).toBe('/Users/a/.bricks/providers.toml');
		expect(providersFilePath('/Users/a/.bricks/')).toBe('/Users/a/.bricks/providers.toml');
		expect(providersFilePath('C:\\Users\\a\\.bricks')).toBe('C:\\Users\\a\\.bricks\\providers.toml');
	});
});
