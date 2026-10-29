// The questions of `Ask` in the conversation state: a question_asked adds one timeline item and
// one waiting question; question_resolved closes both with the outcome; a snapshot restores the
// waiting ones (they have no item until a card is shown for them). Hand-built envelopes.
import { describe, expect, it } from 'vitest';
import { Conversation, type AssistantItem, type QuestionItem } from '../../src/lib/state/conversation.svelte';
import type { QuestionRequest } from '../../src/lib/ipc/engine';
import { EnvelopeFeed, PROJECT_ID, T0, openedSession, snapshot } from './fixtures';

const RUN = 'run-q';

const QUESTION: QuestionRequest = {
	question_id: 'q-1',
	tool_call_id: 'tc-ask-1',
	question: 'Quelle base de données ?',
	options: [{ label: 'Postgres' }, { label: 'SQLite', description: 'fichier local' }],
	recommended: 1,
	timeout_seconds: 60,
	deadline_at: T0 + 60_000
};

function openConversation(): Conversation {
	return new Conversation(openedSession(), PROJECT_ID);
}

function startRun(conv: Conversation, feed: EnvelopeFeed) {
	conv.apply(
		feed.next({ type: 'run_started', prompt: 'Choisis', attachments: [], model: 'anthropic/claude-test' })
	);
}

function questionItems(conv: Conversation): QuestionItem[] {
	return conv.items.filter((i): i is QuestionItem => i.kind === 'question');
}

describe('question_asked: one waiting question and one timeline item', () => {
	it('lists the question as waiting and adds its item to the run', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN });
		startRun(conv, feed);

		conv.apply(feed.next({ type: 'question_asked', question: QUESTION }));

		expect(conv.pendingQuestions).toEqual([QUESTION]);
		expect(conv.questions.get('q-1')).toEqual(QUESTION);
		const [item] = questionItems(conv);
		expect(item.question).toEqual(QUESTION);
		expect(item.outcome).toBeUndefined();
		expect(item.runId).toBe(RUN);
	});

	it('closes the assistant text before the question, so the question follows what came before it', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN });
		startRun(conv, feed);
		conv.apply(feed.next({ type: 'text_delta', text: 'Deux options existent.' }));

		conv.apply(feed.next({ type: 'question_asked', question: QUESTION }));

		expect(conv.items.at(-1)?.kind).toBe('question');
		const assistant = conv.items.find((i): i is AssistantItem => i.kind === 'assistant');
		expect(assistant?.streaming).toBe(false);
	});
});

describe('question_resolved: the question closes with its outcome', () => {
	it('a choice closes the waiting question and sets the outcome on its item', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN });
		startRun(conv, feed);
		conv.apply(feed.next({ type: 'question_asked', question: QUESTION }));

		conv.apply(
			feed.next({
				type: 'question_resolved',
				question_id: 'q-1',
				tool_call_id: 'tc-ask-1',
				outcome: { state: 'chosen', choice: 1 }
			})
		);

		expect(conv.pendingQuestions).toEqual([]);
		expect(questionItems(conv)).toHaveLength(1);
		expect(questionItems(conv)[0].outcome).toEqual({ state: 'chosen', choice: 1 });
	});

	it('keeps a skip with its reason, a timeout included', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN });
		startRun(conv, feed);
		conv.apply(feed.next({ type: 'question_asked', question: QUESTION }));

		conv.apply(
			feed.next({
				type: 'question_resolved',
				question_id: 'q-1',
				tool_call_id: 'tc-ask-1',
				outcome: { state: 'skipped', reason: 'timeout' }
			})
		);

		expect(questionItems(conv)[0].outcome).toEqual({ state: 'skipped', reason: 'timeout' });
		expect(conv.pendingQuestions).toEqual([]);
	});

	it('a close for a question this conversation never showed changes nothing', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN });
		startRun(conv, feed);
		const before = conv.items.length;

		conv.apply(
			feed.next({
				type: 'question_resolved',
				question_id: 'q-unknown',
				tool_call_id: 'tc-x',
				outcome: { state: 'skipped', reason: 'cancelled' }
			})
		);

		expect(conv.items).toHaveLength(before);
		expect(conv.pendingQuestions).toEqual([]);
	});
});

describe('the snapshot restores the waiting questions', () => {
	it('lists a question the snapshot carries, with no timeline item yet (a reload)', () => {
		const conv = openConversation();

		conv.applySnapshot(snapshot({ pending_questions: [QUESTION] }));

		expect(conv.pendingQuestions).toEqual([QUESTION]);
		expect(questionItems(conv)).toEqual([]);
	});

	it('does not duplicate the item of a question already in the timeline', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN });
		startRun(conv, feed);
		conv.apply(feed.next({ type: 'question_asked', question: QUESTION }));

		conv.applySnapshot(snapshot({ activity: { state: 'running', run_id: RUN }, pending_questions: [QUESTION] }));

		expect(questionItems(conv)).toHaveLength(1);
		expect(conv.pendingQuestions).toEqual([QUESTION]);
	});

	it('forgets the waiting questions the snapshot does not carry any more', () => {
		const conv = openConversation();
		conv.applySnapshot(snapshot({ pending_questions: [QUESTION] }));

		conv.applySnapshot(snapshot({ pending_questions: [] }));

		expect(conv.pendingQuestions).toEqual([]);
	});

	it('treats a snapshot from an engine without questions as having none', () => {
		const conv = openConversation();
		conv.applySnapshot(snapshot({ pending_questions: [QUESTION] }));

		const older = { ...snapshot(), pending_questions: undefined } as unknown as ReturnType<typeof snapshot>;
		conv.applySnapshot(older);

		expect(conv.pendingQuestions).toEqual([]);
	});
});
