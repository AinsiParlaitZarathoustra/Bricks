// The responses of a conversation (« Copier »): one copyable group per answer, built from the
// real envelope path (Conversation.apply) and from stored history. Deterministic: no clock.
import { describe, expect, it } from 'vitest';
import type { HistoryMessage, RunRecord } from '../../src/lib/ipc/client';
import type { Envelope, Event as EngineEvent } from '../../src/lib/ipc/engine';
import { Conversation, type AssistantItem, type TimelineItem } from '../../src/lib/state/conversation.svelte';
import { isCopyable, responseByAnchor, responseGroups } from '../../src/lib/features/conversation/responses';
import { EnvelopeFeed, PROJECT_ID, T0, agentResult, openedSession, spawnInfo } from './fixtures';

const RUN_1 = 'run-1';
const RUN_2 = 'run-2';

function openConversation(opened = openedSession()): Conversation {
	return new Conversation(opened, PROJECT_ID);
}

function start(conv: Conversation, feed: EnvelopeFeed, prompt: string, runId: string, at = T0) {
	conv.apply(feed.next({ type: 'run_started', prompt, attachments: [], model: 'anthropic/claude-test' }, { runId, at }));
}

function finish(conv: Conversation, feed: EnvelopeFeed, runId: string, outcome: 'succeeded' | 'cancelled' | 'failed' | 'incomplete' = 'succeeded') {
	conv.apply(feed.next({ type: 'run_finished', outcome, text: '', turns: 1 }, { runId }));
}

function texts(conv: Conversation): string[] {
	return conv.responses.map((g) => g.copyText);
}

/** The assistant items of the timeline that are still streaming. */
function streaming(conv: Conversation): AssistantItem[] {
	return conv.items.filter((i): i is AssistantItem => i.kind === 'assistant' && i.streaming);
}

describe('responses: one copy per answer, from the live events', () => {
	it('an answer with text, a tool, text, a sub-agent and text is one group, copied in order', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		start(conv, feed, 'Analyse le dépôt', RUN_1);
		conv.apply(feed.next({ type: 'text_delta', text: 'Je regarde.' }));
		conv.apply(feed.next({ type: 'tool_started', tool_call_id: 'tc-1', name: 'Read', input: { path: 'a' } }));
		conv.apply(feed.next({ type: 'tool_finished', tool_call_id: 'tc-1', name: 'Read', is_error: false, duration_ms: 3, output: 'a', executed: true, status: 'success' }));
		conv.apply(feed.next({ type: 'text_delta', text: 'Voici' }));
		conv.apply(feed.next({ type: 'text_delta', text: ' la suite.' }));
		// The engine announces a sub-agent with a notice too (it is a timeline item).
		conv.apply(feed.next({ type: 'agent_spawned', agent: spawnInfo({ agent_id: 'ag-1', root_run_id: RUN_1 }) }));
		conv.apply(feed.next({ type: 'notice', message: 'sub-agent ag-1 started' }));
		conv.apply(feed.next({ type: 'agent_tool_started', agent_id: 'ag-1', tool_call_id: 'tc-s', name: 'Read', input: {} }));
		conv.apply(feed.next({ type: 'agent_tool_finished', agent_id: 'ag-1', tool_call_id: 'tc-s', name: 'Read', is_error: false, duration_ms: 2, executed: true, status: 'success', output: 'x', output_truncated: false }));
		conv.apply(feed.next({ type: 'agent_finished', result: agentResult({ agent_id: 'ag-1' }) }));
		conv.apply(feed.next({ type: 'notice', message: 'sub-agent ag-1 finished' }));
		conv.apply(feed.next({ type: 'text_delta', text: 'Fin.' }));
		finish(conv, feed, RUN_1);

		expect(conv.responses).toHaveLength(1);
		const [group] = conv.responses;
		expect(group.copyText).toBe('Je regarde.\n\nVoici la suite.\n\nFin.');
		expect(group.complete).toBe(true);
		expect(group.runId).toBe(RUN_1);
		// The copy button goes right after the run's « bilan » (its run_end item).
		const bilan = conv.items.find((i) => i.kind === 'run_end' && i.runId === RUN_1)!;
		expect(bilan).toBeDefined();
		expect(group.anchorItemId).toBe(bilan.id);
		expect(conv.responseByAnchor(bilan.id)?.key).toBe(RUN_1);
		expect(conv.items.at(-1)?.id).toBe(bilan.id);
	});

	it('a live run with a tool as its last item still anchors on the bilan, not on the tool', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		start(conv, feed, 'Lis', RUN_1);
		conv.apply(feed.next({ type: 'text_delta', text: 'Lecture.' }));
		conv.apply(feed.next({ type: 'tool_started', tool_call_id: 'tc-1', name: 'Read', input: {} }));
		conv.apply(feed.next({ type: 'tool_finished', tool_call_id: 'tc-1', name: 'Read', is_error: false, duration_ms: 1, output: '', executed: true, status: 'success' }));
		finish(conv, feed, RUN_1);

		const bilan = conv.items.find((i) => i.kind === 'run_end' && i.runId === RUN_1)!;
		expect(conv.responses[0].anchorItemId).toBe(bilan.id);
		expect(conv.responseByAnchor(conv.items.findLast((i) => i.kind === 'tool')!.id)).toBeUndefined();
	});

	it('segments ending with line breaks are joined by exactly one blank line, as in the stored record', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		start(conv, feed, 'Crée le fichier', RUN_1);
		conv.apply(feed.next({ type: 'text_delta', text: 'Je crée le fichier.\n\n' }));
		conv.apply(feed.next({ type: 'tool_started', tool_call_id: 'tc-1', name: 'Bash', input: { command: 'ls' } }));
		conv.apply(feed.next({ type: 'tool_finished', tool_call_id: 'tc-1', name: 'Bash', is_error: false, duration_ms: 3, output: 'a', executed: true, status: 'success' }));
		conv.apply(feed.next({ type: 'text_delta', text: '\nFini.\n' }));
		finish(conv, feed, RUN_1);

		expect(texts(conv)).toEqual(['Je crée le fichier.\n\nFini.']);
	});

	it('an edit and a notice in the middle leave no streaming answer behind', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		start(conv, feed, 'Corrige', RUN_1);
		conv.apply(feed.next({ type: 'text_delta', text: 'Avant.' }));
		conv.apply(feed.next({ type: 'edit_applied', tool_call_id: 'tc-e', tool: 'Write', files: [{ path: 'a.txt', kind: 'modify', added: 1, removed: 0, binary: false }] }));
		expect(streaming(conv)).toHaveLength(0);
		conv.apply(feed.next({ type: 'text_delta', text: 'Milieu.' }));
		conv.apply(feed.next({ type: 'notice', message: 'context 80% full' }));
		expect(streaming(conv)).toHaveLength(0);
		conv.apply(feed.next({ type: 'text_delta', text: 'Après.' }));
		finish(conv, feed, RUN_1);

		expect(streaming(conv)).toHaveLength(0);
		expect(texts(conv)).toEqual(['Avant.\n\nMilieu.\n\nAprès.']);
	});

	it('at most one answer streams at a time, and it is the last item', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		start(conv, feed, 'Go', RUN_1);
		const events: EngineEvent[] = [
			{ type: 'text_delta', text: 'a' },
			{ type: 'edit_applied', tool_call_id: 'e1', tool: 'Write', files: [] },
			{ type: 'text_delta', text: 'b' },
			{ type: 'approval_requested', approval: { approval_id: 'ap', tool_call_id: 't', tool: 'Bash', level: 'execute', description: 'd', input: {}, preview: null } as never },
			{ type: 'text_delta', text: 'c' },
		];
		for (const e of events) {
			conv.apply(feed.next(e));
			expect(streaming(conv).length).toBeLessThanOrEqual(1);
			if (streaming(conv).length === 1) expect(conv.items.at(-1)?.kind).toBe('assistant');
		}
	});

	it('two runs answering the same prompt are two responses', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		start(conv, feed, 'Même question', RUN_1);
		conv.apply(feed.next({ type: 'text_delta', text: 'Première réponse.' }, { runId: RUN_1 }));
		finish(conv, feed, RUN_1);
		start(conv, feed, 'Même question', RUN_2);
		conv.apply(feed.next({ type: 'text_delta', text: 'Seconde réponse.' }, { runId: RUN_2 }));
		finish(conv, feed, RUN_2);

		expect(texts(conv)).toEqual(['Première réponse.', 'Seconde réponse.']);
		expect(conv.responses.map((g) => g.runId)).toEqual([RUN_1, RUN_2]);
	});

	it('a typed command copies its answer, not the command', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		conv.setPendingSubmitLabel('/skill debug');
		start(conv, feed, 'Corrige le bug', RUN_1);
		conv.apply(feed.next({ type: 'text_delta', text: 'Bug corrigé.' }));
		finish(conv, feed, RUN_1);

		const user = conv.items.find((i) => i.kind === 'user');
		expect(user?.kind === 'user' && user.commandLabel).toBe('/skill debug');
		expect(texts(conv)).toEqual(['Bug corrigé.']);
	});

	it('an empty answer has no copy button', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		start(conv, feed, 'Rien', RUN_1);
		conv.apply(feed.next({ type: 'thinking_delta', text: 'pensée seulement' }));
		finish(conv, feed, RUN_1);

		expect(conv.responses).toHaveLength(1);
		expect(conv.responses[0].copyText).toBe('');
		expect(isCopyable(conv.responses[0])).toBe(false);
		expect(conv.responseByAnchor(conv.items.at(-2)!.id)).toBeUndefined();
	});

	it('a cancelled or failed run keeps its partial answer, copyable once it has ended', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		start(conv, feed, 'Long', RUN_1);
		conv.apply(feed.next({ type: 'text_delta', text: 'partiel' }));
		finish(conv, feed, RUN_1, 'cancelled');
		expect(conv.responses[0]).toMatchObject({ copyText: 'partiel', complete: true });
		expect(isCopyable(conv.responses[0])).toBe(true);
	});

	it('a run still going is not copyable yet', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		start(conv, feed, 'En cours', RUN_1);
		conv.apply(feed.next({ type: 'text_delta', text: 'Je réfléchis' }));
		expect(conv.responses[0]).toMatchObject({ complete: false, copyText: 'Je réfléchis' });
		expect(isCopyable(conv.responses[0])).toBe(false);
	});

	it('an answer of more than 200 items is copied whole', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		start(conv, feed, 'Beaucoup', RUN_1);
		const parts: string[] = [];
		for (let i = 0; i < 250; i++) {
			conv.apply(feed.next({ type: 'text_delta', text: `p${i}` }));
			parts.push(`p${i}`);
			conv.apply(feed.next({ type: 'tool_started', tool_call_id: `t${i}`, name: 'Read', input: {} }));
			conv.apply(feed.next({ type: 'tool_finished', tool_call_id: `t${i}`, name: 'Read', is_error: false, duration_ms: 1, output: '', executed: true, status: 'success' }));
		}
		finish(conv, feed, RUN_1);
		expect(conv.responses[0].copyText).toBe(parts.join('\n\n'));
		expect(conv.responses[0].assistantItemIds).toHaveLength(250);
	});

	it('the responses are the same after the events are applied again (a replay)', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		const envs: Envelope[] = [
			feed.next({ type: 'run_started', prompt: 'Rejoue', attachments: [], model: 'm' }, { runId: RUN_1 }),
			feed.next({ type: 'text_delta', text: 'Une' }, { runId: RUN_1 }),
			feed.next({ type: 'notice', message: 'n' }, { runId: RUN_1 }),
			feed.next({ type: 'text_delta', text: 'deux' }, { runId: RUN_1 }),
			feed.next({ type: 'run_finished', outcome: 'succeeded', text: '', turns: 1 }, { runId: RUN_1 }),
		];
		for (const e of envs) conv.apply(e);
		const once = conv.responses.map((g) => [g.key, g.copyText, g.anchorItemId]);
		const items = conv.items.length;
		for (const e of envs) conv.apply(e);
		expect(conv.responses.map((g) => [g.key, g.copyText, g.anchorItemId])).toEqual(once);
		expect(conv.items).toHaveLength(items);
		expect(texts(conv)).toEqual(['Une\n\ndeux']);
	});
});

describe('responses: stored history', () => {
	const user = (text: string): HistoryMessage => ({ role: 'user', attachments: [], blocks: [{ type: 'text', text }] });
	const assistant = (...blocks: HistoryMessage['blocks']): HistoryMessage => ({ role: 'assistant', attachments: [], blocks });

	it('a history without metadata is copyable per user message, with no invented summary', () => {
		const conv = openConversation(
			openedSession({
				history: [
					user('Question'),
					assistant({ type: 'text', text: 'Réponse A' }, { type: 'tool_use', id: 'x', name: 'Read', input: {} }),
					{ role: 'user', attachments: [], blocks: [{ type: 'tool_result', tool_use_id: 'x', content: 'ok', is_error: false }] },
					assistant({ type: 'text', text: 'Réponse B' }),
					user('Autre'),
				],
			}),
		);
		expect(conv.responses.map((g) => [g.runId, g.copyText, g.complete])).toEqual([
			[null, 'Réponse A\n\nRéponse B', true],
			[null, '', true],
		]);
		expect(conv.items.some((i) => i.kind === 'run_end')).toBe(false);
	});

	it('a stored run with a record is placed by its user index, with its bilan and its own text', () => {
		const record: RunRecord = {
			version: 1,
			run_id: 'stored-1',
			user_index: 1,
			started_at: T0,
			finished_at: T0 + 2_000,
			outcome: 'succeeded',
			files: [],
			commands: [],
			assistant_text: 'Texte du dossier',
			usage: null,
		};
		const conv = openConversation(
			openedSession({
				history: [
					user('Première'),
					assistant({ type: 'text', text: 'Ancienne réponse' }),
					user('Seconde'),
					assistant({ type: 'text', text: 'Réponse texte visible' }),
				],
				run_records: [record],
			}),
		);
		const groups = conv.responses;
		expect(groups.map((g) => g.runId)).toEqual([null, 'stored-1']);
		expect(groups[1].copyText).toBe('Texte du dossier');
		expect(groups[1].complete).toBe(true);
		expect(conv.run('stored-1')).toMatchObject({ prompt: 'Seconde', workedMs: 2_000, outcome: 'succeeded' });
		expect(conv.items.filter((i) => i.kind === 'run_end').map((i) => i.runId)).toEqual(['stored-1']);
		expect(conv.responseByAnchor(groups[1].anchorItemId!)?.runId).toBe('stored-1');
	});

	it('a stored history without bilan anchors on its last item, with one copy for the response', () => {
		const conv = openConversation(
			openedSession({
				history: [
					user('Question'),
					assistant({ type: 'text', text: 'Début.' }, { type: 'tool_use', id: 'x', name: 'Read', input: {} }),
					{ role: 'user', attachments: [], blocks: [{ type: 'tool_result', tool_use_id: 'x', content: 'ok', is_error: false }] },
					assistant({ type: 'text', text: 'Fin.' }),
				],
			}),
		);
		expect(conv.items.some((i) => i.kind === 'run_end')).toBe(false);
		const last = conv.items.at(-1)!;
		expect(conv.responses).toHaveLength(1);
		expect(conv.responses[0].anchorItemId).toBe(last.id);
		expect(conv.responses[0].copyText).toBe('Début.\n\nFin.');
		const copyable = conv.items.filter((i) => conv.responseByAnchor(i.id) !== undefined);
		expect(copyable.map((i) => i.id)).toEqual([last.id]);
	});

	it('a record without a proven index places nothing and matches no text', () => {
		const record: RunRecord = {
			version: 1,
			run_id: 'stored-2',
			user_index: null,
			started_at: T0,
			finished_at: T0 + 1,
			outcome: 'succeeded',
			files: [],
			commands: [],
			assistant_text: 'Orphelin',
			usage: null,
		};
		const conv = openConversation(
			openedSession({ history: [user('Seconde'), assistant({ type: 'text', text: 'Visible' })], run_records: [record] }),
		);
		expect(conv.run('stored-2')).toBeUndefined();
		expect(conv.responses.map((g) => g.copyText)).toEqual(['Visible']);
		expect(conv.items.some((i) => i.kind === 'run_end')).toBe(false);
	});
});

describe('responseGroups: a pure function of the items', () => {
	it('groups from every item, in order, and anchors each group on its last item', () => {
		const items: TimelineItem[] = [
			{ kind: 'user', id: 'u1', text: 'a', attachments: [] },
			{ kind: 'assistant', id: 'a1', text: 'un', thinking: '', streaming: false },
			{ kind: 'user', id: 'u2', text: 'b', attachments: [] },
			{ kind: 'assistant', id: 'a2', text: '  ', thinking: 'x', streaming: false },
		];
		const groups = responseGroups(items, new Map());
		expect(groups.map((g) => [g.userItemId, g.anchorItemId, g.copyText])).toEqual([
			['u1', 'a1', 'un'],
			['u2', 'a2', ''],
		]);
		expect(responseByAnchor(groups, 'a1')?.copyText).toBe('un');
		expect(responseByAnchor(groups, 'a2')).toBeUndefined();
	});
});
