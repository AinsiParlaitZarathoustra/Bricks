//.2 validation, reducer side: what a tool line and a resolved approval are built from,
// and the context line across a compaction that changed nothing and a clear that is followed
// by a new measure. Hand-built envelopes (fixtures.ts); complements the files it does not repeat.
import { describe, expect, it } from 'vitest';
import { Conversation, type ToolItem } from '../../src/lib/state/conversation.svelte';
import { contextLine } from '../../src/lib/features/context/contextView';
import { approvalTrace, formatToolDuration, toolLineState } from '../../src/lib/features/conversation/timeline/toolLine';
import type { ApprovalRequest } from '../../src/lib/ipc/engine';
import { EnvelopeFeed, PROJECT_ID, T0, openedSession, snapshot } from './fixtures';

const RUN = 'run-validation-1';
const plain = (s: string | null) => (s === null ? null : s.replace(/[\u00a0\u202f]/g, ' '));

function openRun() {
	const conv = new Conversation(openedSession(), PROJECT_ID);
	const feed = new EnvelopeFeed({ runId: RUN });
	conv.apply(feed.next({ type: 'run_started', prompt: 'Teste', attachments: [], model: 'm' }, { at: T0 }));
	return { conv, feed };
}

function toolOf(conv: Conversation): ToolItem {
	const items = conv.items.filter((i): i is ToolItem => i.kind === 'tool');
	if (items.length !== 1) throw new Error(`expected one tool call, got ${items.length}`);
	return items[0];
}

const WRITE_REQUEST = {
	approval_id: 'ap-1',
	tool_call_id: 'tc-w',
	tool: 'Write',
	level: 'write',
	description: 'Écrire notes.txt',
	input: { file_path: '/work/demo/notes.txt', content: 'bonjour' },
	preview: null,
	agent_id: null
} as unknown as ApprovalRequest;

describe('tool line facts from the reducer', () => {
	it('a call finished without duration_ms has no duration: the line shows none', () => {
		// The engine always sends duration_ms (u64); an absent value is handled defensively.
		const { conv, feed } = openRun();
		conv.apply(feed.next({ type: 'tool_started', tool_call_id: 'tc-a', name: 'Grep', input: { pattern: 'x' } }));
		conv.apply(
			feed.next({
				type: 'tool_finished',
				tool_call_id: 'tc-a',
				name: 'Grep',
				is_error: false,
				output: '3 lignes',
				duration_ms: undefined as unknown as number
			})
		);
		const item = toolOf(conv);
		expect(item.durationMs).toBeUndefined();
		expect(formatToolDuration(item.durationMs)).toBeNull();
		expect(toolLineState(item, false)).toBe('done');
	});

	it('the line carries the tool name, never the arguments of the call', () => {
		const { conv, feed } = openRun();
		conv.apply(
			feed.next({ type: 'tool_started', tool_call_id: 'tc-b', name: 'Bash', input: { command: 'cat ~/.ssh/config' } })
		);
		const item = toolOf(conv);
		// The line is built from the name, the state and the duration (ToolRow.svelte): none of
		// the input text is among them.
		expect(item.name).toBe('Bash');
		expect(JSON.stringify({ name: item.name, state: toolLineState(item, false), duration: plain(formatToolDuration(item.durationMs)) }))
			.not.toContain('ssh');
	});

	it('a call still running when its run is cancelled is interrupted, then keeps that state', () => {
		const { conv, feed } = openRun();
		conv.apply(feed.next({ type: 'tool_started', tool_call_id: 'tc-c', name: 'Bash', input: { command: 'sleep 9' } }));
		conv.apply(feed.next({ type: 'run_finished', outcome: 'cancelled', text: '', turns: 1 }));
		const item = toolOf(conv);
		expect(toolLineState(item, conv.run(RUN)?.outcome === 'cancelled')).toBe('interrupted');
	});
});

describe('resolved approval from the reducer', () => {
	it('a request resolved by the person is one trace line', () => {
		const { conv, feed } = openRun();
		conv.apply(feed.next({ type: 'approval_requested', approval: WRITE_REQUEST }));
		conv.apply(
			feed.next({
				type: 'approval_resolved',
				approval_id: 'ap-1',
				tool_call_id: 'tc-w',
				decision: 'deny',
				by: 'user'
			})
		);
		const item = conv.items.find((i) => i.kind === 'approval');
		expect(item && item.kind === 'approval' ? item.resolved : undefined).toEqual({ decision: 'deny', by: 'user' });
		expect(approvalTrace('Write', { decision: 'deny', by: 'user' })).toBe('Refusé · Write · par vous');
		expect(conv.approvals.has('ap-1')).toBe(false);
	});

	it('a request that is still open has no trace yet', () => {
		const { conv, feed } = openRun();
		conv.apply(feed.next({ type: 'approval_requested', approval: WRITE_REQUEST }));
		const item = conv.items.find((i) => i.kind === 'approval');
		expect(item && item.kind === 'approval' ? item.resolved : 'missing').toBeUndefined();
		expect(conv.approvals.has('ap-1')).toBe(true);
	});
});

describe('context line across compaction and clear', () => {
	it('a compaction that compacted nothing keeps the measure fresh', () => {
		const feed = new EnvelopeFeed();
		const conv = new Conversation(openedSession(), PROJECT_ID);
		conv.apply(feed.next({ type: 'compaction', reason: 'manual', outcome: 'ok', compacted: false }));
		expect(conv.contextStale).toBe(false);
		expect(plain(contextLine(conv.context, conv.contextStale))).toBe('0 / 180 000 tokens');
	});

	it('a new measure after a clear shows figures again', () => {
		const feed = new EnvelopeFeed();
		const conv = new Conversation(openedSession(), PROJECT_ID);
		conv.apply(feed.next({ type: 'context_cleared', messages_removed: 2 }));
		expect(contextLine(conv.context, conv.contextStale)).toBe('— / — tokens');
		conv.apply(feed.next({ type: 'context', status: snapshot().context! }));
		expect(conv.contextStale).toBe(false);
		expect(plain(contextLine(conv.context, conv.contextStale))).toBe('0 / 180 000 tokens');
	});
});
