// The cost of a run's response: the engine's total of the run, live and from its stored
// record, and the line that shows it. The total already holds the session agent and its
// sub-agents, so it is taken as it is and replaced, never added to. An older record has no
// cost: it reads as unavailable, never as zero. Deterministic: envelopes built by hand.
import { describe, expect, it } from 'vitest';
import type { RunRecord } from '../../src/lib/ipc/client';
import type { CostEstimate, Usage } from '../../src/lib/ipc/engine';
import { Conversation, type RunUsageView } from '../../src/lib/state/conversation.svelte';
import { EnvelopeFeed, PROJECT_ID, T0, openedSession, usageOf, usageRecord } from './fixtures';

const RUN_1 = 'run-1';

function view(overrides: Partial<RunUsageView> = {}): RunUsageView {
	return { input: 0, output: 0, cacheRead: 0, costUsd: 0.42, partial: false, final: true, ...overrides };
}

function estimate(amount: number, partial = false, unpriced: string[] = []): CostEstimate {
	return { amount_usd: amount, tariff: 'default', partial, unpriced };
}

function openConversation(opened = openedSession()): Conversation {
	return new Conversation(opened, PROJECT_ID);
}

function startRun(conv: Conversation, feed: EnvelopeFeed) {
	conv.apply(feed.next({ type: 'run_started', prompt: 'Lance', attachments: [], model: 'm' }, { runId: RUN_1, at: T0 }));
}

/** The engine's `run_usage` of the run: `total` is the session agent's and its sub-agents' together. */
function runUsage(conv: Conversation, feed: EnvelopeFeed, total: Partial<Usage>, finalTotal: boolean, pending = 0) {
	conv.apply(
		feed.next(
			{
				type: 'run_usage',
				root_run_id: RUN_1,
				own: usageOf(total),
				descendants: usageOf(),
				total: usageOf(total),
				pending_agents: pending,
				final_total: finalTotal,
			},
			{ runId: RUN_1 },
		),
	);
}


describe('the cost of a run, live', () => {
	it('each total of the run replaces its cost, provisional first and final last', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		startRun(conv, feed);
		expect(conv.run(RUN_1)?.usage).toBeNull();

		runUsage(conv, feed, { input_tokens: 1_000, output_tokens: 250, cache_read_input_tokens: 300, cost_usd: 0.2, cost_estimate: estimate(0.2) }, false, 1);
		expect(conv.run(RUN_1)?.usage).toEqual({ input: 1_000, output: 250, cacheRead: 300, costUsd: 0.2, partial: false, final: false });

		runUsage(conv, feed, { input_tokens: 1_500, output_tokens: 300, cache_read_input_tokens: 310, cost_usd: 0.3, cost_estimate: estimate(0.3) }, true, 0);
		expect(conv.run(RUN_1)?.usage).toEqual({ input: 1_500, output: 300, cacheRead: 310, costUsd: 0.3, partial: false, final: true });
	});

	it('a provisional total arriving after the final one does not replace it', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		startRun(conv, feed);
		runUsage(conv, feed, { input_tokens: 1_500, cost_usd: 0.3, cost_estimate: estimate(0.3) }, true);
		runUsage(conv, feed, { input_tokens: 900, cost_usd: 0.1, cost_estimate: estimate(0.1) }, false, 1);
		expect(conv.run(RUN_1)?.usage).toMatchObject({ input: 1_500, costUsd: 0.3, final: true });
	});

	it('the total already holds the sub-agents: it is taken as it is, never added again', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		startRun(conv, feed);
		const own = usageOf({ input_tokens: 1_000, cost_usd: 0.25, cost_estimate: estimate(0.25) });
		const descendants = usageOf({ input_tokens: 500, cost_usd: 0.5, cost_estimate: estimate(0.5) });
		const total = usageOf({ input_tokens: 1_500, cost_usd: 0.75, cost_estimate: estimate(0.75) });
		conv.apply(
			feed.next(
				{ type: 'run_usage', root_run_id: RUN_1, own, descendants, total, pending_agents: 0, final_total: true },
				{ runId: RUN_1 },
			),
		);
		conv.apply(
			feed.next(
				{ type: 'run_usage', root_run_id: RUN_1, own, descendants, total, pending_agents: 0, final_total: true },
				{ runId: RUN_1 },
			),
		);
		expect(conv.run(RUN_1)?.usage).toMatchObject({ input: 1_500, costUsd: 0.75, final: true });
	});

	it('a total without a price is unknown, and a partial estimate is flagged', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		startRun(conv, feed);
		runUsage(conv, feed, { input_tokens: 1_000 }, true);
		expect(conv.run(RUN_1)?.usage).toMatchObject({ input: 1_000, costUsd: null, partial: false });
		runUsage(conv, feed, { input_tokens: 2_000, cost_usd: 0.003, cost_estimate: estimate(0.003, true, ['image']) }, true);
		expect(conv.run(RUN_1)?.usage).toMatchObject({ costUsd: 0.003, partial: true });
	});

	it('the total is kept when the run ends', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		startRun(conv, feed);
		runUsage(conv, feed, { input_tokens: 1_500, cost_usd: 0.3, cost_estimate: estimate(0.3) }, true);
		conv.apply(feed.next({ type: 'run_finished', outcome: 'succeeded', text: 'Fait.', turns: 1 }, { runId: RUN_1 }));
		expect(conv.run(RUN_1)?.usage).toMatchObject({ input: 1_500, costUsd: 0.3, final: true });
	});
});

describe('the cost of a stored run', () => {
	const user = { role: 'user' as const, attachments: [], blocks: [{ type: 'text' as const, text: 'Lance' }] };
	const record = (overrides: Partial<RunRecord> = {}): RunRecord => ({
		version: 1,
		run_id: RUN_1,
		user_index: 0,
		started_at: T0,
		finished_at: T0 + 5_000,
		outcome: 'succeeded',
		files: [],
		commands: [],
		assistant_text: 'Fait.',
		usage: usageRecord(),
		...overrides,
	});

	it('the cost is read back from the record after a reopen', () => {
		const conv = openConversation(
			openedSession({
				history: [user],
				run_records: [record({ usage: usageRecord({ cache_read_input_tokens: 310_000, cost_usd: 0.42 }) })],
			}),
		);
		expect(conv.run(RUN_1)?.usage).toEqual({
			input: 1_500,
			output: 300,
			cacheRead: 310_000,
			costUsd: 0.42,
			partial: false,
			final: true,
		});
	});

	it('a record kept while sub-agents ran is provisional, and the final total replayed after it replaces it', () => {
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		const ring = [
			feed.next({ type: 'run_started', prompt: 'Lance', attachments: [], model: 'm' }, { runId: RUN_1 }),
			feed.next(
				{
					type: 'run_usage',
					root_run_id: RUN_1,
					own: usageOf({ input_tokens: 1_000 }),
					descendants: usageOf({ input_tokens: 600 }),
					total: usageOf({ input_tokens: 1_600, output_tokens: 300, cost_usd: 0.45, cost_estimate: estimate(0.45) }),
					pending_agents: 0,
					final_total: true,
				},
				{ runId: RUN_1 },
			),
			feed.next({ type: 'run_finished', outcome: 'succeeded', text: 'Fait.', turns: 1 }, { runId: RUN_1 }),
		];
		const conv = openConversation(
			openedSession({
				history: [user],
				run_records: [record({ usage: usageRecord({ input_tokens: 1_500, cost_usd: 0.42, final_total: false, pending_agents: 1 }) })],
				recent_events: ring,
				last_seq: 3,
			}),
		);
		expect(conv.run(RUN_1)?.usage).toEqual({ input: 1_600, output: 300, cacheRead: 0, costUsd: 0.45, partial: false, final: true });
	});

	it('a record written before usage was kept has no cost (unknown, never zero)', () => {
		const conv = openConversation(openedSession({ history: [user], run_records: [record({ usage: null })] }));
		expect(conv.run(RUN_1)?.usage).toBeNull();
	});
});
