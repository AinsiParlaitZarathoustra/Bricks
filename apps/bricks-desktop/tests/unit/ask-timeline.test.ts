// The questions of `Ask` in the timeline (ConversationTimeline.svelte), rendered to a string over a
// conversation: a question the timeline has an item for is shown once, in its place; a waiting
// question it has no item for (a reload, a snapshot) is shown after the items, once; a closed
// question with no item shows nothing.
import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import ConversationTimeline from '../../src/lib/features/conversation/ConversationTimeline.svelte';
import { Conversation } from '../../src/lib/state/conversation.svelte';
import type { QuestionRequest } from '../../src/lib/ipc/engine';
import { EnvelopeFeed, PROJECT_ID, T0, openedSession, snapshot } from './fixtures';

const HEADING = 'Requiert votre intervention';

const QUESTION: QuestionRequest = {
	question_id: 'q-1',
	tool_call_id: 'tc-1',
	question: 'Quelle base de données ?',
	options: [{ label: 'Postgres' }, { label: 'SQLite' }],
	recommended: 1,
	timeout_seconds: 60,
	deadline_at: T0 + 60_000
};

function count(html: string, needle: string): number {
	return html.split(needle).length - 1;
}

function renderTimeline(conv: Conversation): string {
	return render(ConversationTimeline, { props: { conversation: conv } }).body;
}

describe('a question the timeline shows', () => {
	it('is shown once, in its place, while it is asked in the run', () => {
		const conv = new Conversation(openedSession(), PROJECT_ID);
		const feed = new EnvelopeFeed({ runId: 'run-t' });
		conv.apply(feed.next({ type: 'run_started', prompt: 'Choisis', attachments: [], model: 'anthropic/claude-test' }));
		conv.apply(feed.next({ type: 'question_asked', question: QUESTION }));

		const html = renderTimeline(conv);

		expect(count(html, HEADING)).toBe(1);
		expect(count(html, 'Quelle base de données ?')).toBeGreaterThanOrEqual(1);
	});

	it('is shown once when the snapshot restores it too: the item is not duplicated', () => {
		const conv = new Conversation(openedSession(), PROJECT_ID);
		const feed = new EnvelopeFeed({ runId: 'run-t' });
		conv.apply(feed.next({ type: 'run_started', prompt: 'Choisis', attachments: [], model: 'anthropic/claude-test' }));
		conv.apply(feed.next({ type: 'question_asked', question: QUESTION }));
		conv.applySnapshot(snapshot({ activity: { state: 'running', run_id: 'run-t' }, pending_questions: [QUESTION] }));

		expect(count(renderTimeline(conv), HEADING)).toBe(1);
	});

	it('a waiting question with no item (after a reload) is shown once, after the items', () => {
		const conv = new Conversation(openedSession(), PROJECT_ID);
		conv.applySnapshot(snapshot({ pending_questions: [QUESTION] }));

		const html = renderTimeline(conv);

		expect(count(html, HEADING)).toBe(1);
		expect(html).toContain('Quelle base de données ?');
	});

	it('a question closed with no item shows nothing', () => {
		const conv = new Conversation(openedSession(), PROJECT_ID);
		const feed = new EnvelopeFeed({ runId: 'run-t' });
		conv.apply(feed.next({ type: 'run_started', prompt: 'Choisis', attachments: [], model: 'anthropic/claude-test' }));
		conv.applySnapshot(snapshot({ pending_questions: [QUESTION] }));
		conv.apply(
			feed.next({
				type: 'question_resolved',
				question_id: 'q-1',
				tool_call_id: 'tc-1',
				outcome: { state: 'skipped', reason: 'cancelled' }
			})
		);

		expect(count(renderTimeline(conv), HEADING)).toBe(0);
	});
});
