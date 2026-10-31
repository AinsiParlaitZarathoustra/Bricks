// The timeline's blocks (runBlocks.ts): the steps of a run gather in one work block under the
// message that started it; once the run has ended, its answer follows the bar and its end follows
// the answer. Items without a run keep their place. Built with the reducer, from the envelopes
// the engine sends, so the items have the shape the view really gets.
import { describe, expect, it } from 'vitest';
import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
import BotIcon from '@lucide/svelte/icons/bot';
import {
	Conversation,
	type ApprovalItem,
	type AssistantItem,
	type QuestionItem,
	type TimelineItem,
} from '../../src/lib/state/conversation.svelte';
import {
	answerOf,
	currentStep,
	needsAttention,
	timelineBlocks,
	type TimelineBlock,
	type WorkBlock,
} from '../../src/lib/features/conversation/runBlocks';
import { MESSAGE_ICON, THINKING_ICON } from '../../src/lib/features/conversation/timeline/toolIcons';
import type { ApprovalRequest, QuestionRequest } from '../../src/lib/ipc/engine';
import { EnvelopeFeed, PROJECT_ID, T0, openedSession } from './fixtures';

const RUN = 'run-blocks';

/** A conversation whose run has started: the run's message is its first item, as the reducer makes it. */
function startedRun() {
	const conv = new Conversation(openedSession(), PROJECT_ID);
	const feed = new EnvelopeFeed({ runId: RUN });
	conv.apply(feed.next({ type: 'run_started', prompt: 'Lis le fichier', attachments: [], model: 'm' }, { at: T0 }));
	return { conv, feed };
}

function readCall(conv: Conversation, feed: EnvelopeFeed, id: string, name = 'Read') {
	conv.apply(feed.next({ type: 'tool_started', tool_call_id: id, name, input: { file_path: 'a.ts' } }));
	conv.apply(feed.next({ type: 'tool_finished', tool_call_id: id, name, is_error: false, duration_ms: 12, output: 'ok' }));
}

function blocksOf(conv: Conversation): TimelineBlock[] {
	return timelineBlocks(conv.items, conv.items, conv.runs, conv.runningRunId);
}

/** A compact reading of the blocks: `work:<steps>`, `answer`, or the kind of an item. */
function shape(blocks: TimelineBlock[]): string[] {
	return blocks.map((b) => (b.kind === 'work' ? `work:${b.items.length}` : b.answer ? 'answer' : b.item.kind));
}

function workOf(blocks: TimelineBlock[], index: number): WorkBlock {
	const block = blocks[index];
	if (block.kind !== 'work') throw new Error(`block ${index} is not a work block`);
	return block;
}

const APPROVAL: ApprovalRequest = {
	approval_id: 'ap-1',
	tool_call_id: 'tc-9',
	tool: 'Bash',
	level: 'execute',
	description: 'Lancer les tests',
	input: { command: 'npm test' },
	preview: null,
};

const QUESTION: QuestionRequest = {
	question_id: 'q-1',
	tool_call_id: 'tc-8',
	question: 'Quelle option ?',
	options: [],
};

describe('timelineBlocks: the steps of a run gather under its message', () => {
	it('a run that has just started is a bar with no step, after its message', () => {
		const { conv } = startedRun();
		const blocks = blocksOf(conv);
		expect(shape(blocks)).toEqual(['user', 'work:0']);
		expect(workOf(blocks, 1)).toMatchObject({ runId: RUN, answerReasoning: null });
	});

	it('a running run keeps all its steps in the bar, the messages between them included', () => {
		const { conv, feed } = startedRun();
		conv.apply(feed.next({ type: 'text_delta', text: 'Je lis le fichier.' }));
		readCall(conv, feed, 'tc-1');
		conv.apply(feed.next({ type: 'tool_started', tool_call_id: 'tc-2', name: 'Grep', input: { pattern: 'x' } }));
		const blocks = blocksOf(conv);
		expect(conv.runningRunId).toBe(RUN);
		expect(shape(blocks)).toEqual(['user', 'work:3']);
		expect(workOf(blocks, 1).items.map((i) => i.kind)).toEqual(['assistant', 'tool', 'tool']);
	});

	it('a finished run moves its answer out of the bar, and its end follows the answer', () => {
		const { conv, feed } = startedRun();
		conv.apply(feed.next({ type: 'text_delta', text: 'Je lis le fichier.' }));
		readCall(conv, feed, 'tc-1');
		conv.apply(feed.next({ type: 'text_delta', text: 'Voici la réponse.' }));
		conv.apply(feed.next({ type: 'run_finished', outcome: 'succeeded', text: 'Voici la réponse.', turns: 2 }));
		const blocks = blocksOf(conv);
		expect(shape(blocks)).toEqual(['user', 'work:2', 'answer', 'run_end']);
		expect(blocks[2]).toMatchObject({ kind: 'item', answer: true, item: { kind: 'assistant', text: 'Voici la réponse.' } });
		// The intermediate message stays with the steps: only the last message with text is the answer.
		expect(workOf(blocks, 1).items.map((i) => i.kind)).toEqual(['assistant', 'tool']);
	});

	it('a run that failed without any text has no answer: its steps, then its end', () => {
		const { conv, feed } = startedRun();
		conv.apply(feed.next({ type: 'tool_started', tool_call_id: 'tc-1', name: 'Bash', input: { command: 'false' } }));
		conv.apply(feed.next({ type: 'tool_finished', tool_call_id: 'tc-1', name: 'Bash', is_error: true, duration_ms: 3, output: 'exit 1' }));
		conv.apply(feed.next({ type: 'run_finished', outcome: 'failed', error: 'Quota', text: '', turns: 1 }));
		expect(shape(blocksOf(conv))).toEqual(['user', 'work:1', 'run_end']);
	});

	it('the answer keeps its reasoning with the steps while it stands apart from them', () => {
		const { conv, feed } = startedRun();
		conv.apply(feed.next({ type: 'thinking_delta', text: 'Je compare les deux options.' }));
		conv.apply(feed.next({ type: 'text_delta', text: 'Je retiens la seconde.' }));
		conv.apply(feed.next({ type: 'run_finished', outcome: 'succeeded', text: 'Je retiens la seconde.', turns: 1 }));
		const blocks = blocksOf(conv);
		const answer = blocks[2];
		expect(answer).toMatchObject({ kind: 'item', answer: true });
		const reasoning = workOf(blocks, 1).answerReasoning;
		expect(reasoning).toMatchObject({ kind: 'assistant', thinking: 'Je compare les deux options.' });
		if (answer.kind !== 'item') throw new Error('the answer is an item');
		expect(reasoning).toBe(answer.item);
	});

	it('a window that starts inside a run still finds its answer, and shows only its own steps', () => {
		const { conv, feed } = startedRun();
		readCall(conv, feed, 'tc-1', 'Read');
		readCall(conv, feed, 'tc-2', 'Grep');
		conv.apply(feed.next({ type: 'text_delta', text: 'Réponse.' }));
		conv.apply(feed.next({ type: 'run_finished', outcome: 'succeeded', text: 'Réponse.', turns: 1 }));
		const from = conv.items.findIndex((i) => i.kind === 'tool' && i.name === 'Grep');
		const blocks = timelineBlocks(conv.items.slice(from), conv.items, conv.runs, conv.runningRunId);
		expect(shape(blocks)).toEqual(['work:1', 'answer', 'run_end']);
		expect(workOf(blocks, 0).items).toEqual([expect.objectContaining({ kind: 'tool', name: 'Grep' })]);
	});

	it('a window that ends before the answer does not show it', () => {
		const { conv, feed } = startedRun();
		conv.apply(feed.next({ type: 'text_delta', text: 'Je lis.' }));
		readCall(conv, feed, 'tc-1');
		conv.apply(feed.next({ type: 'text_delta', text: 'Réponse.' }));
		conv.apply(feed.next({ type: 'run_finished', outcome: 'succeeded', text: 'Réponse.', turns: 1 }));
		const blocks = timelineBlocks(conv.items.slice(0, 3), conv.items, conv.runs, conv.runningRunId);
		expect(shape(blocks)).toEqual(['user', 'work:2']);
	});

	it('items without a run keep their place as they are', () => {
		const conv = new Conversation(openedSession(), PROJECT_ID);
		conv.items.push({ kind: 'user', id: 'old-1', text: 'Ancien message', attachments: [] });
		conv.items.push({ kind: 'notice', id: 'old-2', level: 'info', message: 'Ancienne note' });
		expect(blocksOf(conv)).toEqual([
			{ kind: 'item', key: 'old-1', item: conv.items[0] },
			{ kind: 'item', key: 'old-2', item: conv.items[1] },
		]);
	});

	it('items of a run the timeline does not know keep their place too', () => {
		const conv = new Conversation(openedSession(), PROJECT_ID);
		conv.items.push({ kind: 'user', id: 'ghost-1', runId: 'run-inconnu', text: 'Perdu', attachments: [] });
		expect(shape(blocksOf(conv))).toEqual(['user']);
	});
});

describe('answerOf: the answer of an ended run', () => {
	it('is the last message with text of the run, and none while the run goes on', () => {
		const items: TimelineItem[] = [
			message('a1', 'Premier'),
			message('a2', 'Dernier'),
			{ ...message('a3', 'Autre'), runId: 'run-autre' },
		];
		expect(answerOf(items, RUN, false)).toBe(items[1]);
		expect(answerOf(items, RUN, true)).toBeNull();
	});

	it('is none when the run wrote no text, or only blanks', () => {
		expect(answerOf([message('a1', '   ')], RUN, false)).toBeNull();
		expect(answerOf([], RUN, false)).toBeNull();
	});
});

describe('needsAttention: what asks the person something stays visible', () => {
	const approval = (resolved?: boolean): ApprovalItem => ({
		kind: 'approval',
		id: 'ap',
		runId: RUN,
		approval: APPROVAL,
		...(resolved ? { resolved: { decision: 'allow', by: 'user' } } : {}),
	});
	const question = (answered: boolean): QuestionItem => ({
		kind: 'question',
		id: 'qu',
		runId: RUN,
		question: QUESTION,
		...(answered ? { outcome: { state: 'skipped', reason: 'cancelled' } } : {}),
	});

	it('an approval waiting for an answer, and a question still open, need the person', () => {
		expect(needsAttention(approval(false))).toBe(true);
		expect(needsAttention(question(false))).toBe(true);
	});

	it('a decided approval or an answered question no longer does', () => {
		expect(needsAttention(approval(true))).toBe(false);
		expect(needsAttention(question(true))).toBe(false);
	});

	it('the steps and the messages never do', () => {
		const items: TimelineItem[] = [
			{ kind: 'user', id: 'u', text: 'x', attachments: [] },
			{ kind: 'tool', id: 't', toolCallId: 'tc', name: 'Bash', input: {}, status: 'running' },
			message('a'),
		];
		expect(items.map(needsAttention)).toEqual([false, false, false]);
	});
});

function message(id: string, text = '', thinking = '', streaming = false): AssistantItem {
	return { kind: 'assistant', id, runId: RUN, text, thinking, streaming };
}

describe('currentStep: what the run does now', () => {
	it('is none before any step', () => {
		expect(currentStep([])).toBeNull();
		expect(currentStep([{ kind: 'user', id: 'u', text: 'x', attachments: [] }])).toBeNull();
	});

	it('is the tool that runs, with its own icon and name', () => {
		const items: TimelineItem[] = [{ kind: 'tool', id: 't', toolCallId: 'tc', name: 'Bash', input: {}, status: 'running' }];
		expect(currentStep(items)).toEqual({ icon: ChevronRightIcon, label: 'Bash' });
	});

	it('is the last tool when several ran', () => {
		const items: TimelineItem[] = [
			{ kind: 'tool', id: 't1', toolCallId: 'tc1', name: 'Read', input: {}, status: 'ok' },
			{ kind: 'tool', id: 't2', toolCallId: 'tc2', name: 'Grep', input: {}, status: 'running' },
		];
		expect(currentStep(items)?.label).toBe('Grep');
	});

	it('is a sub-agent at work', () => {
		const items: TimelineItem[] = [{ kind: 'agent', id: 'g', agentId: 'ag-1', execution: 1 }];
		expect(currentStep(items)).toEqual({ icon: BotIcon, label: 'Agent' });
	});

	it('is « Rédige » once the run writes a message, even after a tool', () => {
		const items: TimelineItem[] = [
			{ kind: 'tool', id: 't', toolCallId: 'tc', name: 'Read', input: {}, status: 'ok' },
			message('a', 'Je rédige'),
		];
		expect(currentStep(items)).toEqual({ icon: MESSAGE_ICON, label: 'Rédige' });
	});

	it('is « Réfléchit » while the run reasons, streamed or already reasoned', () => {
		expect(currentStep([message('a', '', 'Je pense', true)])).toEqual({ icon: THINKING_ICON, label: 'Réfléchit' });
		expect(currentStep([message('a', '', 'Je pense', false)])).toEqual({ icon: THINKING_ICON, label: 'Réfléchit' });
	});

	it('skips an empty message and an approval, and finds the tool before them', () => {
		const items: TimelineItem[] = [
			{ kind: 'tool', id: 't', toolCallId: 'tc', name: 'Read', input: {}, status: 'ok' },
			message('a'),
			{ kind: 'approval', id: 'ap', approval: APPROVAL },
		];
		expect(currentStep(items)?.label).toBe('Read');
	});
});
