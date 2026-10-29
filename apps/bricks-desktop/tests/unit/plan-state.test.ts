// The session plan (TodoWrite) in the conversation state: a plan is adopted only when its
// revision is newer than the one held, whatever the order of the snapshot and of the replayed
// events; a cleared plan is a newer revision, so it is never resurrected by an older one.
import { describe, expect, it } from 'vitest';
import { Conversation } from '../../src/lib/state/conversation.svelte';
import type { Envelope, PlanState, PlanStep, Snapshot } from '../../src/lib/ipc/engine';
import { EnvelopeFeed, PROJECT_ID, SESSION_ID, T0, openedSession, snapshot } from './fixtures';

const OTHER_SESSION_ID = 'sess-other';

const STEPS: PlanStep[] = [
	{ content: 'Lire le code', status: 'completed', activeForm: 'Lecture du code' },
	{ content: 'Écrire les tests', status: 'in_progress', activeForm: 'Écriture des tests' }
];

/** A plan at `revision`, with the steps given (none: a cleared plan). */
function plan(revision: number, todos: PlanStep[] = STEPS): PlanState {
	return { version: 1, revision, todos, updated_at: T0 + revision, explanation: `révision ${revision}` };
}

/** A conversation opened on a snapshot holding `held` (none when omitted), with nothing replayed. */
function openWith(held?: PlanState, extra: { recent?: Envelope[]; lastSeq?: number } = {}): Conversation {
	const snap: Snapshot = held ? snapshot({ plan: held }) : snapshot();
	return new Conversation(
		openedSession({ snapshot: snap, recent_events: extra.recent ?? [], last_seq: extra.lastSeq ?? 0 }),
		PROJECT_ID
	);
}

describe('Conversation.applyPlan: the newest revision wins', () => {
	it('an older replayed update does not replace the snapshot plan', () => {
		const conv = openWith(plan(3));
		const feed = new EnvelopeFeed();
		conv.apply(feed.next({ type: 'plan_updated', plan: plan(2) }));
		expect(conv.plan?.revision).toBe(3);
	});

	it('a newer update replaces the held plan', () => {
		const conv = openWith(plan(3));
		const feed = new EnvelopeFeed();
		conv.apply(feed.next({ type: 'plan_updated', plan: plan(4, [STEPS[0]!]) }));
		expect(conv.plan?.revision).toBe(4);
		expect(conv.plan?.todos).toHaveLength(1);
	});

	it('an empty plan at a newer revision replaces a non-empty one (a clear)', () => {
		const conv = openWith(plan(4));
		const feed = new EnvelopeFeed();
		conv.apply(feed.next({ type: 'plan_updated', plan: plan(5, []) }));
		expect(conv.plan).toEqual(plan(5, []));
	});

	it('a stale update of the old plan does not resurrect it after a clear', () => {
		const conv = openWith(undefined);
		const feed = new EnvelopeFeed();
		conv.apply(feed.next({ type: 'plan_updated', plan: plan(5, []) }));
		conv.apply(feed.next({ type: 'plan_updated', plan: plan(4) }));
		expect(conv.plan?.revision).toBe(5);
		expect(conv.plan?.todos).toEqual([]);
	});

	it('an older snapshot taken later does not replace a newer plan', () => {
		const conv = openWith(plan(3));
		conv.applySnapshot(snapshot({ plan: plan(2) }));
		expect(conv.plan?.revision).toBe(3);
	});

	it('revision 0 is never adopted, and never replaces a held plan', () => {
		const empty = openWith(undefined);
		empty.applyPlan({ ...plan(0), todos: [] });
		expect(empty.plan).toBeNull();

		const held = openWith(plan(3));
		held.applyPlan({ ...plan(0), todos: [] });
		expect(held.plan?.revision).toBe(3);
	});

	it('an engine snapshot without a plan (an older engine) leaves the plan null', () => {
		const conv = openWith(undefined);
		expect(conv.plan).toBeNull();
		conv.applySnapshot(snapshot());
		expect(conv.plan).toBeNull();
	});

	it('the plan of a snapshot is the one held when no event came after it', () => {
		const conv = openWith(plan(3));
		expect(conv.plan).toEqual(plan(3));
	});
});

describe('Conversation.apply: envelopes of plan_updated', () => {
	it('an envelope of another session never changes this plan, nor its sequence', () => {
		const conv = openWith(plan(3));
		const foreign = {
			...new EnvelopeFeed().next({ type: 'plan_updated', plan: plan(9, []) }),
			session_id: OTHER_SESSION_ID,
			seq: 100
		} as Envelope;
		conv.apply(foreign);
		expect(conv.plan?.revision).toBe(3);

		// Its high sequence number did not move this conversation's: its own envelope still applies.
		conv.apply(new EnvelopeFeed().next({ type: 'plan_updated', plan: plan(4) }));
		expect(conv.plan?.revision).toBe(4);
		expect(conv.sessionId).toBe(SESSION_ID);
	});

	it('a duplicated sequence number is ignored, even with a newer revision', () => {
		const conv = openWith(undefined);
		const feed = new EnvelopeFeed();
		const first = feed.next({ type: 'plan_updated', plan: plan(1) });
		conv.apply(first);
		expect(conv.plan?.revision).toBe(1);
		conv.apply({ ...first, plan: plan(2) } as Envelope);
		expect(conv.plan?.revision).toBe(1);
	});

	it('an update already in the snapshot (sequence at or below last_seq) is ignored', () => {
		const conv = openWith(undefined, { lastSeq: 5 });
		// Sequence 5 is what the snapshot already holds: its newer plan is not applied.
		const held = { ...new EnvelopeFeed().next({ type: 'plan_updated', plan: plan(7) }), seq: 5 } as Envelope;
		conv.apply(held);
		expect(conv.plan).toBeNull();
		conv.apply({ ...held, seq: 6 } as Envelope);
		expect(conv.plan?.revision).toBe(7);
	});

	it('a replayed update older than the snapshot does not show at open', () => {
		const feed = new EnvelopeFeed();
		const recent = [feed.next({ type: 'plan_updated', plan: plan(2, []) })];
		const conv = openWith(plan(3), { recent, lastSeq: 0 });
		expect(conv.plan?.revision).toBe(3);
		expect(conv.plan?.todos).toHaveLength(2);
	});
});
