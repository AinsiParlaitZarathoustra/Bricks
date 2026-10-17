// Unit tests for the context line « ≈ 12 374 / 1 000 000 tokens » (contextView.ts), and
// for the reducer events that make the measure stale. Hand-built envelopes (fixtures.ts).
import { describe, expect, it } from 'vitest';
import { Conversation } from '../../src/lib/state/conversation.svelte';
import type { ContextStatus, Provenance } from '../../src/lib/ipc/engine';
import { contextFigures, contextLine } from '../../src/lib/features/context/contextView';
import { EnvelopeFeed, MODEL, PROJECT_ID, openedSession, snapshot } from './fixtures';

/** Intl's fr-FR grouping uses a narrow no-break space; the product allows it. Tests compare plain spaces. */
const plain = (s: string) => s.replace(/[  ]/g, ' ');

function measure(overrides: Partial<ContextStatus> = {}, provenance: Provenance = 'measured'): ContextStatus {
	return {
		context_used: {
			tokens: 12_374,
			upper_bound: 12_374,
			measured_tokens: provenance === 'estimated' ? 0 : 12_374,
			estimated_tokens: provenance === 'measured' || provenance === 'counted' ? 0 : 1_200,
			provenance,
			method: 'fixture'
		},
		context_window: { model: MODEL, total: 1_000_000, max_input_tokens: 900_000, max_output_tokens: 8_000 },
		input_limit: 900_000,
		reserved_output: 8_000,
		margin: 0,
		total_tokens: 5_000_000,
		totals: {
			input_tokens: 4_000_000,
			cache_read_tokens: 0,
			cache_write_tokens: 0,
			output_tokens: 900_000,
			reasoning_tokens: 0,
			requests: 40,
			requests_without_usage: 0,
			compaction_requests: 0
		},
		notes: [],
		...overrides
	};
}

describe('context line: numerator and denominator', () => {
	it('shows the occupation over the model window, in French format and qualified by ≈', () => {
		expect(plain(contextLine(measure(), false))).toBe('≈ 12 374 / 1 000 000 tokens');
	});

	it.each<Provenance>(['measured', 'counted', 'mixed', 'estimated'])(
		'uses the occupation as numerator for %s provenance',
		(provenance) => {
			const status = measure({}, provenance);
			expect(contextFigures(status, false).used).toBe(12_374);
			expect(plain(contextLine(status, false))).toBe('≈ 12 374 / 1 000 000 tokens');
		}
	);

	it('never takes the cumulative billed totals as numerator', () => {
		const status = measure({ total_tokens: 9_999_999 });
		expect(plain(contextLine(status, false))).toBe('≈ 12 374 / 1 000 000 tokens');
		expect(plain(contextLine(status, false))).not.toContain('9 999 999');
	});

	it('does not replace an unknown window by the input limit', () => {
		const status = measure({ context_window: { model: MODEL, total: null, max_input_tokens: 900_000, max_output_tokens: null } });
		expect(plain(contextLine(status, false))).toBe('≈ 12 374 / — tokens');
		expect(plain(contextLine(status, false))).not.toContain('900 000');
	});

	it('treats a zero window as unknown', () => {
		const status = measure({ context_window: { model: MODEL, total: 0, max_input_tokens: 0, max_output_tokens: null } });
		expect(contextFigures(status, false).total).toBeNull();
		expect(plain(contextLine(status, false))).toBe('≈ 12 374 / — tokens');
	});
});

describe('context line: missing values', () => {
	it('shows dashes for both figures when there is no measure yet', () => {
		expect(contextLine(null, false)).toBe('— / — tokens');
		expect(contextFigures(null, false)).toEqual({ used: null, total: null });
	});

	it('shows a dash for the numerator when the engine gave no occupation', () => {
		const status = measure();
		(status.context_used as unknown as Record<string, unknown>).tokens = undefined;
		expect(plain(contextLine(status, false))).toBe('— / 1 000 000 tokens');
	});

	it('rejects a non-finite or negative occupation', () => {
		expect(contextFigures(measure({ context_used: { ...measure().context_used, tokens: Number.NaN } }), false).used).toBeNull();
		expect(contextFigures(measure({ context_used: { ...measure().context_used, tokens: -1 } }), false).used).toBeNull();
	});
});

describe('context line: stale measure', () => {
	it('shows dashes for both figures while the measure is stale', () => {
		expect(contextLine(measure(), true)).toBe('— / — tokens');
		expect(contextFigures(measure(), true)).toEqual({ used: null, total: null });
	});

	it('a model change makes the shown value stale until the next context event', () => {
		const feed = new EnvelopeFeed();
		const conv = new Conversation(openedSession({ snapshot: snapshot({ context: measure() }) }), PROJECT_ID);
		expect(plain(contextLine(conv.context, conv.contextStale))).toBe('≈ 12 374 / 1 000 000 tokens');

		conv.apply(feed.next({ type: 'model_changed', model: 'anthropic/other-model', applies: 'next_run' }));
		expect(conv.contextStale).toBe(true);
		expect(contextLine(conv.context, conv.contextStale)).toBe('— / — tokens');

		const fresh = measure({
			context_used: { tokens: 500, upper_bound: 500, measured_tokens: 500, estimated_tokens: 0, provenance: 'counted', method: 'fixture' },
			context_window: { model: 'anthropic/other-model', total: 200_000, max_input_tokens: 180_000, max_output_tokens: 8_000 }
		});
		conv.apply(feed.next({ type: 'context', status: fresh }));
		expect(conv.contextStale).toBe(false);
		expect(plain(contextLine(conv.context, conv.contextStale))).toBe('≈ 500 / 200 000 tokens');
	});

	it('a cleared context is stale until the next measure', () => {
		const feed = new EnvelopeFeed();
		const conv = new Conversation(openedSession({ snapshot: snapshot({ context: measure() }) }), PROJECT_ID);
		conv.apply(feed.next({ type: 'context_cleared', messages_removed: 4 }));
		expect(contextLine(conv.context, conv.contextStale)).toBe('— / — tokens');
	});

	it('a compaction that compacted makes the value stale', () => {
		const feed = new EnvelopeFeed();
		const conv = new Conversation(openedSession({ snapshot: snapshot({ context: measure() }) }), PROJECT_ID);
		conv.apply(feed.next({ type: 'compaction', reason: 'manual', outcome: 'ok', compacted: true }));
		expect(contextLine(conv.context, conv.contextStale)).toBe('— / — tokens');
	});
});
