// Formats of the edit tools in the desktop: `Edit` with `edits` (several replacements in one
// call), the single-replacement `Edit`, and `MultiEdit`, the historical name kept as recorded.
// Display logic only: the summary, the name of the call, the approval trace. Nothing is rewritten.
import { describe, expect, it } from 'vitest';
import type { HistoryMessage } from '../../src/lib/ipc/client';
import { summarizeInput } from '../../src/lib/features/conversation/format';
import { approvalTrace, toolLineState } from '../../src/lib/features/conversation/timeline/toolLine';
import { Conversation, type ToolItem } from '../../src/lib/state/conversation.svelte';
import { EnvelopeFeed, PROJECT_ID, T0, openedSession } from './fixtures';

const PATH = 'src/lib/features/a.ts';
const RUN = 'run-edit-1';

const MULTI_INPUT = {
	file_path: PATH,
	edits: [
		{ old_string: 'let a = 1;', new_string: 'let a = 10;' },
		{ old_string: 'foo', new_string: 'bar', replace_all: true }
	]
};

const SINGLE_INPUT = { file_path: PATH, old_string: 'let a = 1;', new_string: 'let a = 10;' };

function toolItems(items: Conversation['items']): ToolItem[] {
	return items.filter((i): i is ToolItem => i.kind === 'tool');
}

describe('summarizeInput of the edit tools', () => {
	it('an Edit with edits is summarised by its path, as a single replacement is', () => {
		expect(summarizeInput('Edit', MULTI_INPUT)).toBe(PATH);
		expect(summarizeInput('Edit', SINGLE_INPUT)).toBe(PATH);
	});

	it('the operations of an Edit with edits do not leak into the one-line summary', () => {
		const summary = summarizeInput('Edit', MULTI_INPUT);
		expect(summary).not.toContain('old_string');
		expect(summary).not.toContain('edits');
	});

	it('a historical MultiEdit line keeps its path', () => {
		expect(summarizeInput('MultiEdit', MULTI_INPUT)).toBe(PATH);
	});

	it('an Edit with edits and no path stays a bounded one-line summary', () => {
		const summary = summarizeInput('Edit', { edits: MULTI_INPUT.edits });
		expect(summary.length).toBeLessThanOrEqual(160);
		expect(summary).not.toContain('\n');
	});
});

describe('the call of an edit tool in the timeline', () => {
	function openRun() {
		const conv = new Conversation(openedSession(), PROJECT_ID);
		const feed = new EnvelopeFeed({ runId: RUN });
		conv.apply(feed.next({ type: 'run_started', prompt: 'Édite', attachments: [], model: 'm' }, { at: T0 }));
		return { conv, feed };
	}

	it('an Edit with edits keeps its name and its path as summary, and is done once it succeeds', () => {
		const { conv, feed } = openRun();
		conv.apply(feed.next({ type: 'tool_started', tool_call_id: 'tc-1', name: 'Edit', input: MULTI_INPUT }));
		conv.apply(
			feed.next({ type: 'tool_finished', tool_call_id: 'tc-1', name: 'Edit', is_error: false, duration_ms: 12, output: 'ok' })
		);
		const [item] = toolItems(conv.items);
		expect(item.name).toBe('Edit');
		expect(summarizeInput(item.name, item.input)).toBe(PATH);
		expect(toolLineState(item, false)).toBe('done');
	});

	it('an Edit with edits that fails is failed, with the engine output kept', () => {
		const { conv, feed } = openRun();
		conv.apply(feed.next({ type: 'tool_started', tool_call_id: 'tc-2', name: 'Edit', input: MULTI_INPUT }));
		conv.apply(
			feed.next({
				type: 'tool_finished',
				tool_call_id: 'tc-2',
				name: 'Edit',
				is_error: true,
				duration_ms: 3,
				output: 'old_string introuvable'
			})
		);
		const [item] = toolItems(conv.items);
		expect(item.name).toBe('Edit');
		expect(item.output).toBe('old_string introuvable');
		expect(toolLineState(item, false)).toBe('failed');
	});

	it('a MultiEdit call of a live run keeps its recorded name', () => {
		const { conv, feed } = openRun();
		conv.apply(feed.next({ type: 'tool_started', tool_call_id: 'tc-3', name: 'MultiEdit', input: MULTI_INPUT }));
		conv.apply(
			feed.next({ type: 'tool_finished', tool_call_id: 'tc-3', name: 'MultiEdit', is_error: false, duration_ms: 5, output: 'ok' })
		);
		const [item] = toolItems(conv.items);
		expect(item.name).toBe('MultiEdit');
		expect(summarizeInput(item.name, item.input)).toBe(PATH);
	});

	it('a historical MultiEdit tool_use is displayed as stored: same name, same input, no alias', () => {
		const history = [
			({
				role: 'assistant',
				blocks: [{ type: 'tool_use', id: 'hist-1', name: 'MultiEdit', input: MULTI_INPUT }]
			}) as HistoryMessage
		];
		const conv = new Conversation(openedSession({ history }), PROJECT_ID);
		const items = toolItems(conv.items);
		expect(items).toHaveLength(1);
		expect(items[0].name).toBe('MultiEdit');
		expect(items[0].input).toEqual(MULTI_INPUT);
		expect(items[0].status).toBe('ok');
		expect(summarizeInput(items[0].name, items[0].input)).toBe(PATH);
	});
});

describe('the approval trace of the edit tools', () => {
	it('names an Edit as the approval gave it', () => {
		expect(approvalTrace('Edit', { decision: 'allow', by: 'user' })).toBe('Autorisé · Edit · par vous');
	});

	it('names a MultiEdit as the approval gave it, without renaming it', () => {
		expect(approvalTrace('MultiEdit', { decision: 'deny', by: 'user' })).toBe('Refusé · MultiEdit · par vous');
	});
});
