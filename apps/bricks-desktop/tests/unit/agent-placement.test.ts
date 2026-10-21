// Where a sub-agent sits in the thread (Conversation.apply and hydrateAgents), with hand-built
// envelopes: after the parent's call that launched it and its siblings, before the run end when
// it has no call, at the end otherwise; a resume is a second item; a stale execution changes nothing.
import { describe, expect, it } from 'vitest';
import { Conversation, type TimelineItem } from '../../src/lib/state/conversation.svelte';
import type { InstanceRecord } from '../../src/lib/ipc/engine';
import { EnvelopeFeed, PROJECT_ID, T0, agentResult, openedSession, spawnInfo } from './fixtures';

const RUN_1 = 'run-1';

function openConversation(): Conversation {
	return new Conversation(openedSession(), PROJECT_ID);
}

function startRun(conv: Conversation, feed: EnvelopeFeed) {
	conv.apply(feed.next({ type: 'run_started', prompt: 'Cherche', attachments: [], model: 'anthropic/claude-test' }, { at: T0 }));
}

function toolStarted(conv: Conversation, feed: EnvelopeFeed, toolCallId: string, name = 'Agents') {
	conv.apply(feed.next({ type: 'tool_started', tool_call_id: toolCallId, name, input: {} }));
}

function finishRun(conv: Conversation, feed: EnvelopeFeed) {
	conv.apply(
		feed.next({ type: 'run_finished', outcome: 'succeeded', text: 'Fait.', turns: 1 }, { at: T0 + 9_000 })
	);
}

/** The thread as a list of short labels, in order. The run's prompt (a user item) is left out. */
function layout(items: readonly TimelineItem[]): string[] {
	return items.filter((item) => item.kind !== 'user').map((item) => {
		switch (item.kind) {
			case 'tool':
				return `tool:${item.toolCallId}`;
			case 'agent':
				return `agent:${item.agentId}#${item.execution}`;
			case 'run_end':
				return 'end';
			default:
				return item.kind;
		}
	});
}

describe('sub-agents in the thread: placement', () => {
	it('is placed right after its launching call, after the siblings of the same call', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		startRun(conv, feed);

		toolStarted(conv, feed, 'tc-A');
		conv.apply(feed.next({ type: 'agent_spawned', agent: spawnInfo({ agent_id: 'ag-1', tool_call_id: 'tc-A' }) }));
		conv.apply(feed.next({ type: 'agent_spawned', agent: spawnInfo({ agent_id: 'ag-2', tool_call_id: 'tc-A' }) }));
		toolStarted(conv, feed, 'tc-B', 'Read');
		// A later sibling of tc-A still goes before the call that came after it.
		conv.apply(feed.next({ type: 'agent_spawned', agent: spawnInfo({ agent_id: 'ag-3', tool_call_id: 'tc-A' }) }));

		expect(layout(conv.items)).toEqual([
			'tool:tc-A',
			'agent:ag-1#1',
			'agent:ag-2#1',
			'agent:ag-3#1',
			'tool:tc-B'
		]);
		expect(conv.items.find((i) => i.id === 'agent-ag-1')).toMatchObject({ kind: 'agent', agentId: 'ag-1', execution: 1, toolCallId: 'tc-A' });
	});

	it('starts an agent at execution 1 with its own identity', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		startRun(conv, feed);
		toolStarted(conv, feed, 'tc-A');
		conv.apply(feed.next({ type: 'agent_spawned', agent: spawnInfo({ agent_id: 'ag-1', tool_call_id: 'tc-A' }) }, { at: T0 + 100 }));

		expect(conv.agents.get('ag-1')).toMatchObject({ execution: 1, spawnedAt: T0 + 100 });
	});

	it('places an agent with no launching call before the run end, and after it when its run is unknown', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		startRun(conv, feed);
		conv.apply(feed.next({ type: 'agent_spawned', agent: spawnInfo({ agent_id: 'ag-4' }) }));
		finishRun(conv, feed);
		expect(layout(conv.items)).toEqual(['agent:ag-4#1', 'end']);

		// A background agent of the same finished run goes before the « bilan », not after it.
		conv.apply(feed.next({ type: 'agent_spawned', agent: spawnInfo({ agent_id: 'ag-5' }) }));
		expect(layout(conv.items)).toEqual(['agent:ag-4#1', 'agent:ag-5#1', 'end']);

		// A run this thread does not know: the end of the thread.
		conv.apply(
			feed.next({ type: 'agent_spawned', agent: spawnInfo({ agent_id: 'ag-6', root_run_id: 'run-unknown' }) })
		);
		expect(layout(conv.items)).toEqual(['agent:ag-4#1', 'agent:ag-5#1', 'end', 'agent:ag-6#1']);
	});

	it('does not add a second item for the same agent when it is spawned again', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		startRun(conv, feed);
		toolStarted(conv, feed, 'tc-A');
		conv.apply(feed.next({ type: 'agent_spawned', agent: spawnInfo({ agent_id: 'ag-1', tool_call_id: 'tc-A' }) }));
		conv.apply(feed.next({ type: 'agent_spawned', agent: spawnInfo({ agent_id: 'ag-1', tool_call_id: 'tc-A' }) }));

		expect(layout(conv.items)).toEqual(['tool:tc-A', 'agent:ag-1#1']);
	});
});

describe('sub-agents in the thread: resume and stale executions', () => {
	/** An agent that completed in its first execution, with a result, and a later tool call. */
	function completedAgent() {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		startRun(conv, feed);
		toolStarted(conv, feed, 'tc-A');
		conv.apply(feed.next({ type: 'agent_spawned', agent: spawnInfo({ agent_id: 'ag-1', tool_call_id: 'tc-A' }) }, { at: T0 + 100 }));
		conv.apply(feed.next({ type: 'agent_state', agent_id: 'ag-1', state: 'completed' }, { at: T0 + 2_000 }));
		conv.apply(feed.next({ type: 'agent_finished', result: agentResult({ agent_id: 'ag-1' }) }, { at: T0 + 2_500 }));
		toolStarted(conv, feed, 'tc-B', 'Read');
		return { conv, feed };
	}

	it('a resume starts execution 2 at the end of the thread, timed from now, with its own item', () => {
		const { conv, feed } = completedAgent();

		conv.apply(feed.next({ type: 'agent_state', agent_id: 'ag-1', state: 'running', execution: 2 }, { at: T0 + 5_000 }));

		expect(layout(conv.items)).toEqual(['tool:tc-A', 'agent:ag-1#1', 'tool:tc-B', 'agent:ag-1#2']);
		expect(conv.agents.get('ag-1')).toMatchObject({
			state: 'running',
			execution: 2,
			spawnedAt: T0 + 5_000,
			endedAt: undefined,
			result: undefined,
			tools: []
		});
		expect(conv.items.find((i) => i.id === 'agent-ag-1-e2')).toMatchObject({ kind: 'agent', execution: 2 });
		// The first item keeps its own identity: it is not moved or renamed.
		expect(conv.items.find((i) => i.id === 'agent-ag-1')).toMatchObject({ execution: 1, toolCallId: 'tc-A' });
	});

	it('a second state of the same resumed execution adds no third item', () => {
		const { conv, feed } = completedAgent();
		conv.apply(feed.next({ type: 'agent_state', agent_id: 'ag-1', state: 'running', execution: 2 }, { at: T0 + 5_000 }));
		conv.apply(feed.next({ type: 'agent_state', agent_id: 'ag-1', state: 'queued', execution: 2 }, { at: T0 + 5_100 }));

		expect(layout(conv.items)).toEqual(['tool:tc-A', 'agent:ag-1#1', 'tool:tc-B', 'agent:ag-1#2']);
		expect(conv.agents.get('ag-1')).toMatchObject({ state: 'queued', execution: 2 });
	});

	it('a state or a result of an older execution changes nothing', () => {
		const { conv, feed } = completedAgent();
		conv.apply(feed.next({ type: 'agent_state', agent_id: 'ag-1', state: 'running', execution: 2 }, { at: T0 + 5_000 }));
		const before = { ...conv.agents.get('ag-1')! };
		const itemsBefore = layout(conv.items);

		// A late « completed » of execution 1, and its late result.
		conv.apply(feed.next({ type: 'agent_state', agent_id: 'ag-1', state: 'completed', execution: 1 }, { at: T0 + 6_000 }));
		conv.apply(
			feed.next(
				{ type: 'agent_finished', result: agentResult({ agent_id: 'ag-1', execution: 1, summary: 'Ancien.' }) },
				{ at: T0 + 6_100 }
			)
		);

		const agent = conv.agents.get('ag-1');
		expect(agent).toMatchObject({ state: 'running', execution: 2, spawnedAt: T0 + 5_000, endedAt: undefined });
		expect(agent?.result).toBeUndefined();
		expect(agent?.state).toBe(before.state);
		expect(layout(conv.items)).toEqual(itemsBefore);
	});

	it('an execution-1 state with no field is stale once execution 2 runs', () => {
		const { conv, feed } = completedAgent();
		conv.apply(feed.next({ type: 'agent_state', agent_id: 'ag-1', state: 'running', execution: 2 }));
		conv.apply(feed.next({ type: 'agent_state', agent_id: 'ag-1', state: 'failed' }, { at: T0 + 7_000 }));

		expect(conv.agents.get('ag-1')).toMatchObject({ state: 'running', execution: 2, endedAt: undefined });
	});

	it('a result of the current execution is kept', () => {
		const { conv, feed } = completedAgent();
		conv.apply(feed.next({ type: 'agent_state', agent_id: 'ag-1', state: 'running', execution: 2 }, { at: T0 + 5_000 }));
		conv.apply(
			feed.next(
				{ type: 'agent_finished', result: agentResult({ agent_id: 'ag-1', execution: 2, summary: 'Nouveau.' }) },
				{ at: T0 + 6_000 }
			)
		);

		expect(conv.agents.get('ag-1')?.result?.summary).toBe('Nouveau.');
	});
});

describe('sub-agents in the thread: hydrated after a reopen', () => {
	function record(overrides: Partial<InstanceRecord['info']> = {}, extra: Partial<InstanceRecord> = {}): InstanceRecord {
		return {
			info: spawnInfo(overrides),
			state: 'completed',
			background: false,
			updated_at_ms: T0 + 500,
			...extra
		};
	}

	it('places a restored agent after its launching call and an orphan before the run end', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		startRun(conv, feed);
		toolStarted(conv, feed, 'tc-H');
		finishRun(conv, feed);

		conv.hydrateAgents(
			[
				record({ agent_id: 'ag-h', tool_call_id: 'tc-H' }),
				record({ agent_id: 'ag-o' })
			],
			[]
		);

		expect(layout(conv.items)).toEqual(['tool:tc-H', 'agent:ag-h#1', 'agent:ag-o#1', 'end']);
	});

	it('restores a resumed agent as its current execution and keeps a live agent untouched', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		startRun(conv, feed);
		toolStarted(conv, feed, 'tc-H');
		conv.apply(feed.next({ type: 'agent_spawned', agent: spawnInfo({ agent_id: 'ag-h', tool_call_id: 'tc-H' }) }));
		const live = conv.agents.get('ag-h');

		conv.hydrateAgents(
			[
				record({ agent_id: 'ag-h', tool_call_id: 'tc-H' }, { execution: 2, state: 'running' }),
				record({ agent_id: 'ag-r' }, { execution: 2, state: 'running' })
			],
			[]
		);

		expect(conv.agents.get('ag-h')).toBe(live);
		expect(conv.agents.get('ag-r')).toMatchObject({ execution: 2, state: 'running' });
		expect(layout(conv.items)).toEqual(['tool:tc-H', 'agent:ag-h#1', 'agent:ag-r#2']);
	});

	it('gives a restored agent without a number the execution 1', () => {
		const conv = openConversation();
		conv.hydrateAgents([record({ agent_id: 'ag-n' })], []);

		expect(conv.agents.get('ag-n')?.execution).toBe(1);
		expect(layout(conv.items)).toEqual(['agent:ag-n#1']);
	});
});
