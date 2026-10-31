// The context card's figures and sources (contextView.ts): formatShort writes a token count short
// (`812`, `33k`, `284.9k`, `1M`), and contextShares splits the occupation by source from the
// characters of the last request. Pure functions; statuses are built from the fixtures.
import { describe, expect, it } from 'vitest';
import { contextShares, formatShort } from '../../src/lib/features/context/contextView';
import type { ContextChars, ContextStatus } from '../../src/lib/ipc/engine';
import { snapshot } from './fixtures';

const NONE: ContextChars = { instructions: 0, conversation: 0, tool_results: 0, tool_definitions: 0, other: 0 };

/** A status whose occupation is `used` and whose last request counted the given characters. */
function statusOf(used: number, chars?: Partial<ContextChars>): ContextStatus {
	const base = snapshot().context;
	return {
		...base,
		context_used: { ...base.context_used, tokens: used },
		input_limit: 1_000_000,
		...(chars ? { breakdown: { turn: 2, chars: { ...NONE, ...chars } } } : {})
	};
}

describe('formatShort: a token count, short', () => {
	it('keeps a count under a thousand as it is, rounded to the token', () => {
		expect(formatShort(812)).toBe('812');
		expect(formatShort(812.4)).toBe('812');
		expect(formatShort(812.6)).toBe('813');
		expect(formatShort(0)).toBe('0');
	});

	it('reads thousands with one decimal, and drops a zero decimal', () => {
		expect(formatShort(33_000)).toBe('33k');
		expect(formatShort(284_900)).toBe('284.9k');
		expect(formatShort(1_000)).toBe('1k');
		expect(formatShort(1_500)).toBe('1.5k');
		expect(formatShort(999_940)).toBe('999.9k');
	});

	it('reads millions with M, and drops a zero decimal', () => {
		expect(formatShort(1_000_000)).toBe('1M');
		expect(formatShort(1_200_000)).toBe('1.2M');
	});

	// Known defect of contextView.ts: the figure is rounded after the division, so 999 950 reads
	// `1000k` where `1M` is expected. It passes while the defect is present; turn it into `it`
	// once formatShort rounds to the next unit.
	it('999 950 reads 1M, not 1000k', () => {
		expect(formatShort(999_950)).toBe('1M');
	});
});

describe('contextShares: no sources', () => {
	it('is null when the status carries no breakdown (an older engine)', () => {
		expect(contextShares(statusOf(100_000), false)).toBeNull();
	});

	it('is null without a status', () => {
		expect(contextShares(null, false)).toBeNull();
	});

	it('is null when the figures are stale', () => {
		expect(contextShares(statusOf(100_000, { conversation: 10 }), true)).toBeNull();
	});

	it('is null when no text was counted (the total is zero)', () => {
		expect(contextShares(statusOf(100_000, {}), false)).toBeNull();
	});

	it('is null when the occupation is unknown', () => {
		expect(contextShares(statusOf(-1, { conversation: 10 }), false)).toBeNull();
	});

	it('is null when one source is not a count', () => {
		expect(contextShares(statusOf(100_000, { conversation: Number.NaN }), false)).toBeNull();
	});
});

describe('contextShares: the parts and the tokens', () => {
	const counted = { conversation: 300, tool_results: 100, tool_definitions: 400, instructions: 150, other: 50 };

	it('gives each source its part of the counted text', () => {
		const shares = contextShares(statusOf(100_000, counted), false)!;
		const byName = Object.fromEntries(shares.map((s) => [s.source, s.share]));
		expect(byName.conversation).toBeCloseTo(0.3);
		expect(byName.tool_results).toBeCloseTo(0.1);
		expect(byName.tool_definitions).toBeCloseTo(0.4);
		expect(byName.instructions).toBeCloseTo(0.15);
		expect(byName.other).toBeCloseTo(0.05);
	});

	it('the parts add up to 1, and the tokens to the occupation', () => {
		const shares = contextShares(statusOf(100_000, counted), false)!;
		expect(shares.reduce((sum, s) => sum + s.share, 0)).toBeCloseTo(1);
		expect(shares.reduce((sum, s) => sum + s.tokens, 0)).toBeCloseTo(100_000);
	});

	it('the tokens of a source are the occupation times its part', () => {
		const shares = contextShares(statusOf(100_000, counted), false)!;
		const definitions = shares.find((s) => s.source === 'tool_definitions')!;
		expect(definitions.tokens).toBeCloseTo(40_000);
	});

	it('leaves out the sources that counted no text', () => {
		const shares = contextShares(statusOf(349_600, { conversation: 600, tool_results: 400 }), false)!;
		expect(shares.map((s) => s.source)).toEqual(['conversation', 'tool_results']);
	});

	it('lists the sources in the same order, whatever the order they were counted in', () => {
		const shares = contextShares(
			statusOf(1_000, { other: 1, instructions: 1, tool_definitions: 1, tool_results: 1, conversation: 1 }),
			false
		)!;
		expect(shares.map((s) => s.source)).toEqual([
			'conversation',
			'tool_results',
			'tool_definitions',
			'instructions',
			'other'
		]);
	});

	it('names each source in French', () => {
		const shares = contextShares(
			statusOf(1_000, { conversation: 1, tool_results: 1, tool_definitions: 1, instructions: 1, other: 1 }),
			false
		)!;
		expect(shares.map((s) => s.label)).toEqual([
			'Messages',
			'Sorties d’outils',
			'Définitions',
			'Prompt système',
			'Autre'
		]);
	});
});
