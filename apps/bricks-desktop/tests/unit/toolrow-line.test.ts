// Unit tests for the one-line tool call (timeline ToolRow / ToolLine) and the resolved
// approval trace: state, interruption, duration in ms. Reducer cases use hand-built envelopes.
import { describe, expect, it } from 'vitest';
import { Conversation, type ToolItem } from '../../src/lib/state/conversation.svelte';
import {
	TOOL_LINE_LABEL,
	approvalTrace,
	formatToolDuration,
	toolLineState
} from '../../src/lib/features/conversation/timeline/toolLine';
import { EnvelopeFeed, PROJECT_ID, T0, openedSession } from './fixtures';

const RUN = 'run-tool-1';

/** Narrow no-break spaces from fr-FR grouping are allowed in the product; tests compare plain spaces. */
const plain = (s: string | null) => (s === null ? null : s.replace(/[  ]/g, ' '));

function toolItems(conv: Conversation): ToolItem[] {
	return conv.items.filter((i): i is ToolItem => i.kind === 'tool');
}

/** The state ToolRow derives for a call: the interruption flag comes from the run's outcome. */
function stateOf(conv: Conversation, item: ToolItem): ReturnType<typeof toolLineState> {
	return toolLineState(item, conv.run(item.runId)?.outcome === 'cancelled');
}

/** The only tool call of a conversation built in a test. */
function onlyTool(conv: Conversation): ToolItem {
	const items = toolItems(conv);
	if (items.length !== 1) throw new Error(`expected one tool call, got ${items.length}`);
	return items[0];
}

describe('toolLineState', () => {
	it('is running while the call runs, and done once it succeeds', () => {
		expect(toolLineState({ status: 'running' }, false)).toBe('running');
		expect(toolLineState({ status: 'ok', durationMs: 42, output: 'ok' }, false)).toBe('done');
	});

	it('is failed when the call returned an error with its result', () => {
		expect(toolLineState({ status: 'error', durationMs: 120, output: 'boom' }, false)).toBe('failed');
	});

	it('is interrupted when the owning run was cancelled and the call never reported back', () => {
		expect(toolLineState({ status: 'error' }, true)).toBe('interrupted');
		expect(toolLineState({ status: 'running' }, true)).toBe('interrupted');
	});

	it('keeps a call that finished before the cancellation as failed', () => {
		expect(toolLineState({ status: 'error', durationMs: 120, output: 'boom' }, true)).toBe('failed');
	});

	it('is failed, not interrupted, when the run was not cancelled', () => {
		expect(toolLineState({ status: 'error' }, false)).toBe('failed');
	});

	it('uses French labels', () => {
		expect(TOOL_LINE_LABEL).toEqual({
			running: 'En cours',
			done: 'Terminé',
			failed: 'Échec',
			interrupted: 'Interrompu'
		});
	});
});

describe('formatToolDuration', () => {
	it('formats seconds and subsecond durations', () => {
		expect(plain(formatToolDuration(6_100))).toBe('6s');
		expect(formatToolDuration(42.4)).toBe('42ms');
		expect(formatToolDuration(0)).toBe('0ms');
	});

	it('gives nothing when the engine reported no duration', () => {
		expect(formatToolDuration(undefined)).toBeNull();
		expect(formatToolDuration(Number.NaN)).toBeNull();
		expect(formatToolDuration(-5)).toBeNull();
	});

	it('converts longer durations to minutes and hours', () => {
		expect(plain(formatToolDuration(90_000))).toBe('1m 30s');
		expect(formatToolDuration(2_000)).toBe('2s');
		expect(formatToolDuration(109_000)).toBe('1m 49s');
		expect(formatToolDuration(5_088_000)).toBe('1h 24m 48s');
		expect(formatToolDuration(60_000)).toBe('1m');
		expect(formatToolDuration(3_600_000)).toBe('1h');
	});
});

describe('approvalTrace', () => {
	it('reads as decision, tool, and who decided', () => {
		expect(approvalTrace('Bash', { decision: 'allow', by: 'user' })).toBe('Autorisé · Bash · par vous');
		expect(approvalTrace('Write', { decision: 'deny', by: 'user' })).toBe('Refusé · Write · par vous');
		expect(approvalTrace('Edit', { decision: 'deny', by: 'non_interactive' })).toBe(
			'Refusé · Edit · mode non interactif'
		);
	});

	it('shows a stored value of the former session grant as an allow, without a cause', () => {
		// Conversations stored before answers covered one call only.
		const stored = { decision: 'allow_for_session', by: 'session' } as unknown as {
			decision: 'allow';
			by: 'user';
		};
		expect(approvalTrace('Edit', stored)).toBe('Autorisé · Edit');
	});
});

describe('tool calls in a reducer-built timeline', () => {
	function openRun() {
		const conv = new Conversation(openedSession(), PROJECT_ID);
		const feed = new EnvelopeFeed({ runId: RUN });
		conv.apply(feed.next({ type: 'run_started', prompt: 'Teste', attachments: [], model: 'm' }, { at: T0 }));
		return { conv, feed };
	}

	it('a finished call is done with its engine duration', () => {
		const { conv, feed } = openRun();
		conv.apply(feed.next({ type: 'tool_started', tool_call_id: 'tc-1', name: 'Bash', input: { command: 'npm test' } }));
		conv.apply(
			feed.next({ type: 'tool_finished', tool_call_id: 'tc-1', name: 'Bash', is_error: false, duration_ms: 6_100, output: 'ok' })
		);
		const item = onlyTool(conv);
		expect(stateOf(conv, item)).toBe('done');
		expect(plain(formatToolDuration(item.durationMs))).toBe('6s');
	});

	it('a call still running has no duration and reads as running', () => {
		const { conv, feed } = openRun();
		conv.apply(feed.next({ type: 'tool_started', tool_call_id: 'tc-2', name: 'Read', input: { path: 'a.md' } }));
		const item = onlyTool(conv);
		expect(stateOf(conv, item)).toBe('running');
		expect(formatToolDuration(item.durationMs)).toBeNull();
	});

	it('a call unfinished when its run is cancelled reads as interrupted', () => {
		const { conv, feed } = openRun();
		conv.apply(feed.next({ type: 'tool_started', tool_call_id: 'tc-3', name: 'Bash', input: { command: 'sleep 60' } }));
		conv.apply(feed.next({ type: 'run_finished', outcome: 'cancelled', text: '', turns: 1 }));
		const item = onlyTool(conv);
		expect(conv.run(RUN)?.outcome).toBe('cancelled');
		expect(item.status).toBe('error');
		expect(stateOf(conv, item)).toBe('interrupted');
	});

	it('a call that failed before the cancellation keeps its failure', () => {
		const { conv, feed } = openRun();
		conv.apply(feed.next({ type: 'tool_started', tool_call_id: 'tc-4', name: 'Bash', input: { command: 'false' } }));
		conv.apply(
			feed.next({ type: 'tool_finished', tool_call_id: 'tc-4', name: 'Bash', is_error: true, duration_ms: 15, output: 'exit 1' })
		);
		conv.apply(feed.next({ type: 'run_finished', outcome: 'cancelled', text: '', turns: 1 }));
		expect(stateOf(conv, onlyTool(conv))).toBe('failed');
	});

	it('a call unfinished when a run fails is failed, not interrupted', () => {
		const { conv, feed } = openRun();
		conv.apply(feed.next({ type: 'tool_started', tool_call_id: 'tc-5', name: 'Edit', input: {} }));
		conv.apply(feed.next({ type: 'run_finished', outcome: 'failed', failure: 'error', error: 'Quota', text: '', turns: 1 }));
		expect(stateOf(conv, onlyTool(conv))).toBe('failed');
	});
});
