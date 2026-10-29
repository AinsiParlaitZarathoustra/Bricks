// Unit tests of the composer ring and its figures (contextView.ts): contextGauge, gaugeLine and
// cacheHitRate. Pure functions; the status is hand-built. Intl's fr-FR uses no-break spaces, so
// the figures are compared after replacing them by plain spaces.
import { describe, expect, it } from 'vitest';
import type { ContextStatus } from '../../src/lib/ipc/engine';
import { cacheHitRate, contextGauge, gaugeLine } from '../../src/lib/features/context/contextView';

const plain = (s: string) => s.replace(/[  ]/g, ' ');

function status(used: number | undefined, total: number | null, totals: Partial<ContextStatus['totals']> = {}): ContextStatus {
	return {
		context_used: {
			tokens: used,
			upper_bound: used ?? 0,
			measured_tokens: used ?? 0,
			estimated_tokens: 0,
			provenance: 'measured',
			method: 'fixture'
		} as unknown as ContextStatus['context_used'],
		context_window: { model: 'fixture/model', max_input_tokens: total, max_output_tokens: null } as unknown as ContextStatus['context_window'],
		input_limit: total,
		reserved_output: 8_000,
		margin: 0,
		total_tokens: 0,
		totals: {
			input_tokens: 0,
			cache_read_tokens: 0,
			cache_write_tokens: 0,
			output_tokens: 0,
			reasoning_tokens: 0,
			requests: 0,
			requests_without_usage: 0,
			compaction_requests: 0,
			...totals
		},
		notes: []
	} as ContextStatus;
}

describe('contextGauge: the ring', () => {
	it('an empty occupation is a real zero: fill 0, 0 %', () => {
		const g = contextGauge(status(0, 1_000_000), false);
		expect(g).toEqual({ used: 0, total: 1_000_000, ratio: 0, percent: 0 });
		expect(plain(gaugeLine(g))).toBe('0 / 1 M (0,0 %)');
	});

	it('an unknown occupation or no status is neutral: no fill, no percentage', () => {
		for (const g of [contextGauge(null, false), contextGauge(status(undefined, 1_000_000), false)]) {
			expect(g.ratio).toBeNull();
			expect(g.percent).toBeNull();
		}
		expect(plain(gaugeLine(contextGauge(null, false)))).toBe('— / —');
	});

	it('a stale measure is neutral even when its figures are there', () => {
		const g = contextGauge(status(655_000, 1_000_000), true);
		expect(g).toEqual({ used: null, total: null, ratio: null, percent: null });
		expect(plain(gaugeLine(g))).toBe('— / —');
	});

	it('an unknown window keeps the occupation and gives no share', () => {
		const g = contextGauge(status(12_374, null), false);
		expect(g.ratio).toBeNull();
		expect(g.percent).toBeNull();
		expect(plain(gaugeLine(g))).toBe('12,4 k / —');
	});

	it('a share above the window: the drawn fill stops at 1, the real percentage is kept', () => {
		const g = contextGauge(status(1_200_000, 1_000_000), false);
		expect(g.ratio).toBe(1);
		expect(g.percent).toBeCloseTo(120, 10);
		expect(plain(gaugeLine(g))).toBe('1,2 M / 1 M (120,0 %)');
	});

	it('a share inside the window is drawn as it is', () => {
		const g = contextGauge(status(655_000, 1_000_000), false);
		expect(g.ratio).toBeCloseTo(0.655, 10);
		expect(g.percent).toBeCloseTo(65.5, 10);
		expect(plain(gaugeLine(g))).toBe('655 k / 1 M (65,5 %)');
	});

	it('a window of zero is an unknown window', () => {
		const g = contextGauge(status(100, 0), false);
		expect(g.total).toBeNull();
		expect(g.ratio).toBeNull();
	});

	it('the drawn fill is always within 0 to 1', () => {
		for (const used of [0, 1, 999_999, 1_000_000, 5_000_000]) {
			const { ratio } = contextGauge(status(used, 1_000_000), false);
			expect(ratio).toBeGreaterThanOrEqual(0);
			expect(ratio).toBeLessThanOrEqual(1);
		}
	});
});

describe('gaugeLine: the ring panel value', () => {
	it('uses compact French figures with the share in parentheses', () => {
		const g = contextGauge(status(686_100, 1_000_000), false);
		expect(plain(gaugeLine(g))).toBe('686,1 k / 1 M (68,6 %)');
	});

	it('drops the share when it is unknown, keeping the two figures', () => {
		const g = contextGauge(status(2_500_000, null), false);
		expect(plain(gaugeLine(g))).toBe('2,5 M / —');
		expect(gaugeLine(g)).not.toContain('%');
	});
});

describe('cacheHitRate: share of prompt tokens read from the cache', () => {
	it('is the cache read over uncached input + cache read + cache write', () => {
		const rate = cacheHitRate(status(1, 1, { input_tokens: 100, cache_read_tokens: 300, cache_write_tokens: 100 }));
		expect(rate).toBeCloseTo(60, 10);
	});

	it('counts output tokens nowhere', () => {
		const without = cacheHitRate(status(1, 1, { input_tokens: 100, cache_read_tokens: 300, cache_write_tokens: 100 }));
		const with_output = cacheHitRate(
			status(1, 1, { input_tokens: 100, cache_read_tokens: 300, cache_write_tokens: 100, output_tokens: 50_000 })
		);
		expect(with_output).toBe(without);
	});

	it('no prompt token counted gives no rate, even with output tokens', () => {
		expect(cacheHitRate(status(1, 1, { output_tokens: 999 }))).toBeNull();
		expect(cacheHitRate(null)).toBeNull();
	});

	it('a prompt made only of cache writes is a rate of zero, not an unknown one', () => {
		expect(cacheHitRate(status(1, 1, { cache_write_tokens: 10 }))).toBe(0);
	});

	it('a prompt entirely read from the cache is 100 %', () => {
		expect(cacheHitRate(status(1, 1, { cache_read_tokens: 50 }))).toBe(100);
	});
});
