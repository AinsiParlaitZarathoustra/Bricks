// Unit tests for the conversation reducer (Conversation.apply) with hand-built envelopes.
import { describe, expect, it } from 'vitest';
import { Conversation, type AssistantItem, type TimelineItem, type ToolItem } from '../../src/lib/state/conversation.svelte';
import { EnvelopeFeed, PROJECT_ID, T0, agentResult, openedSession, spawnInfo } from './fixtures';

const RUN_1 = 'run-1';
const RUN_2 = 'run-2';

function openConversation(): Conversation {
	return new Conversation(openedSession(), PROJECT_ID);
}

function startRun(conv: Conversation, feed: EnvelopeFeed, prompt: string, at = T0) {
	conv.apply(
		feed.next(
			{ type: 'run_started', prompt, attachments: [], model: 'anthropic/claude-test' },
			{ at }
		)
	);
}

/** Returns the item at `index`, failing the test with a clear message when the kind differs. */
function itemOfKind<K extends TimelineItem['kind']>(
	items: TimelineItem[],
	index: number,
	kind: K
): Extract<TimelineItem, { kind: K }> {
	const item = items[index];
	if (item?.kind !== kind) {
		throw new Error(`expected item ${index} to be "${kind}", got "${item?.kind ?? 'nothing'}"`);
	}
	return item as Extract<TimelineItem, { kind: K }>;
}

function toolItems(conv: Conversation): ToolItem[] {
	return conv.items.filter((i): i is ToolItem => i.kind === 'tool');
}

describe('Conversation.apply: text streaming', () => {
	it('streams deltas into one assistant item and closes it at run end', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });

		startRun(conv, feed, 'Dis bonjour');
		conv.apply(feed.next({ type: 'text_delta', text: 'Bonjour' }));
		conv.apply(feed.next({ type: 'text_delta', text: ' !' }));

		expect(conv.items.map((i) => i.kind)).toEqual(['user', 'assistant']);
		expect(conv.running).toBe(true);
		expect(conv.runningRunId).toBe(RUN_1);

		const assistant = itemOfKind(conv.items, 1, 'assistant') as AssistantItem;
		expect(assistant.text).toBe('Bonjour !');
		expect(assistant.streaming).toBe(true);

		conv.apply(
			feed.next({
				type: 'run_finished',
				outcome: 'succeeded',
				text: 'Bonjour !',
				turns: 1
			})
		);

		expect(assistant.streaming).toBe(false);
		expect(conv.running).toBe(false);
		expect(conv.items.at(-1)?.kind).toBe('run_end');
	});

	it('starts a new assistant item after a tool call', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });

		startRun(conv, feed, 'Lis le README');
		conv.apply(feed.next({ type: 'text_delta', text: 'Je lis le fichier.' }));
		conv.apply(
			feed.next({ type: 'tool_started', tool_call_id: 'tc-0', name: 'Read', input: { path: 'README.md' } })
		);
		conv.apply(feed.next({ type: 'text_delta', text: 'Voici le contenu.' }));

		expect(conv.items.map((i) => i.kind)).toEqual(['user', 'assistant', 'tool', 'assistant']);
		expect((conv.items[1] as AssistantItem).text).toBe('Je lis le fichier.');
		expect((conv.items[1] as AssistantItem).streaming).toBe(false);
		expect((conv.items[3] as AssistantItem).text).toBe('Voici le contenu.');
	});
});

describe('Conversation.apply: tools', () => {
	it('tracks a tool from start, through progress, to a successful finish', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });

		startRun(conv, feed, 'Lis');
		conv.apply(feed.next({ type: 'tool_started', tool_call_id: 'tc-1', name: 'Read', input: { path: 'a.md' } }));

		const [started] = toolItems(conv);
		expect(started?.status).toBe('running');
		expect(started?.toolCallId).toBe('tc-1');

		conv.apply(feed.next({ type: 'tool_progress', tool_call_id: 'tc-1', name: 'Read', message: 'lecture de 120 lignes' }));
		expect(toolItems(conv)[0]?.progress).toBe('lecture de 120 lignes');

		conv.apply(
			feed.next({ type: 'tool_finished', tool_call_id: 'tc-1', name: 'Read', is_error: false, duration_ms: 42, output: '# Bricks' })
		);

		const [finished] = toolItems(conv);
		expect(finished?.status).toBe('ok');
		expect(finished?.durationMs).toBe(42);
		expect(finished?.output).toBe('# Bricks');
		expect(finished?.progress).toBeUndefined();
	});

	it('marks a failed shell command as an error and keeps it as command evidence', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });

		startRun(conv, feed, 'Teste');
		conv.apply(feed.next({ type: 'tool_started', tool_call_id: 'tc-2', name: 'Bash', input: { command: 'npm test' } }));
		conv.apply(
			feed.next({
				type: 'tool_finished',
				tool_call_id: 'tc-2',
				name: 'Bash',
				is_error: true,
				duration_ms: 1500,
				output: 'Error: 2 tests failed'
			})
		);

		expect(toolItems(conv)[0]?.status).toBe('error');
		const run = conv.run(RUN_1);
		expect(run?.commands).toHaveLength(1);
		expect(run?.commands[0]).toMatchObject({
			toolCallId: 'tc-2',
			command: 'npm test',
			isError: true,
			durationMs: 1500,
			output: 'Error: 2 tests failed'
		});
	});

	it('closes a tool that never finished when the run ends', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });

		startRun(conv, feed, 'Modifie');
		conv.apply(feed.next({ type: 'tool_started', tool_call_id: 'tc-3', name: 'Edit', input: {} }));
		conv.apply(feed.next({ type: 'run_finished', outcome: 'failed', failure: 'error', error: 'Interrompu', text: '', turns: 1 }));

		expect(toolItems(conv)[0]?.status).toBe('error');
		expect(conv.run(RUN_1)?.error).toBe('Interrompu');
	});
});

describe('Conversation.apply: approvals', () => {
	it('adds a pending approval, then records the decision on its item', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });

		startRun(conv, feed, 'Installe');
		conv.apply(
			feed.next({
				type: 'approval_requested',
				approval: {
					approval_id: 'ap-1',
					tool_call_id: 'tc-4',
					tool: 'Bash',
					level: 'exec',
					description: 'npm install',
					input: { command: 'npm install' },
					preview: null
				}
			})
		);

		expect(conv.pendingApprovals.map((a) => a.approval_id)).toEqual(['ap-1']);
		const pending = itemOfKind(conv.items, 1, 'approval');
		expect(pending.resolved).toBeUndefined();

		conv.apply(feed.next({ type: 'approval_resolved', approval_id: 'ap-1', tool_call_id: 'tc-4', decision: 'deny', by: 'user' }));

		expect(conv.pendingApprovals).toHaveLength(0);
		expect(pending.resolved).toEqual({ decision: 'deny', by: 'user' });
	});
});

describe('Conversation.apply: edits', () => {
	it('sums applied edits per path within a run, separately for each run', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });

		startRun(conv, feed, 'Corrige', T0);
		conv.apply(
			feed.next({
				type: 'edit_applied',
				tool_call_id: 'tc-5',
				tool: 'Edit',
				files: [{ path: 'src/a.ts', kind: 'modify', added: 3, removed: 1, binary: false }]
			})
		);
		conv.apply(
			feed.next({
				type: 'edit_applied',
				tool_call_id: 'tc-6',
				tool: 'Write',
				files: [
					{ path: 'src/a.ts', kind: 'modify', added: 2, removed: 0, binary: false },
					{ path: 'README.md', kind: 'create', added: 10, removed: 0, binary: false }
				]
			})
		);

		const run1 = conv.run(RUN_1);
		expect(run1?.files.size).toBe(2);
		expect(run1?.files.get('src/a.ts')).toEqual({
			path: 'src/a.ts',
			added: 5,
			removed: 1,
			binary: false,
			operations: 2
		});
		expect(run1?.files.get('README.md')).toMatchObject({ added: 10, removed: 0, operations: 1 });
		expect(conv.items.filter((i) => i.kind === 'edit')).toHaveLength(2);

		// A second run starts its own totals.
		conv.apply(feed.next({ type: 'run_finished', outcome: 'succeeded', text: '', turns: 1 }));
		conv.apply(feed.next({ type: 'run_started', prompt: 'Encore', attachments: [], model: 'm' }, { runId: RUN_2, at: T0 + 5_000 }));
		conv.apply(
			feed.next(
				{
					type: 'edit_applied',
					tool_call_id: 'tc-7',
					tool: 'Edit',
					files: [{ path: 'src/a.ts', kind: 'modify', added: 1, removed: 1, binary: false }]
				},
				{ runId: RUN_2 }
			)
		);

		expect(conv.run(RUN_2)?.files.get('src/a.ts')).toMatchObject({ added: 1, removed: 1, operations: 1 });
		expect(conv.run(RUN_1)?.files.get('src/a.ts')?.added).toBe(5);
	});
});

describe('Conversation.apply: run outcome and duration', () => {
	it('records outcome, final text, turns and the duration from engine timestamps', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });

		startRun(conv, feed, 'Résume', T0);
		conv.apply(feed.next({ type: 'text_delta', text: 'Terminé.' }, { at: T0 + 1_000 }));
		conv.apply(
			feed.next(
				{ type: 'run_finished', outcome: 'succeeded', text: 'Terminé.', turns: 2 },
				{ at: T0 + 4_200 }
			)
		);

		const run = conv.run(RUN_1);
		expect(run?.outcome).toBe('succeeded');
		expect(run?.finalText).toBe('Terminé.');
		expect(run?.turns).toBe(2);
		expect((run?.finishedAt ?? NaN) - (run?.startedAt ?? NaN)).toBe(4_200);
		expect(conv.running).toBe(false);
		expect(conv.runningRunId).toBeNull();
	});

	it('keeps a failed run distinct from a succeeded one', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });

		startRun(conv, feed, 'Échoue');
		conv.apply(
			feed.next({ type: 'run_finished', outcome: 'failed', failure: 'error', error: 'Quota dépassé', text: '', turns: 0 })
		);

		expect(conv.run(RUN_1)).toMatchObject({ outcome: 'failed', error: 'Quota dépassé' });
	});
});

describe('Conversation.apply: sequencing', () => {
	it('ignores a duplicate or older envelope (same or lower seq)', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });

		const started = feed.next({ type: 'run_started', prompt: 'Salut', attachments: [], model: 'm' });
		conv.apply(started);
		const delta = feed.next({ type: 'text_delta', text: 'Bonjour' });
		conv.apply(delta);
		conv.apply(delta);
		conv.apply(started);

		const assistant = itemOfKind(conv.items, 1, 'assistant');
		expect(assistant.text).toBe('Bonjour');
		expect(conv.items).toHaveLength(2);
		expect(conv.lastSeq).toBe(delta.seq);
	});
});

describe('Conversation.apply: sub-agents', () => {
	it('moves an agent through its states and records its end', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });

		startRun(conv, feed, 'Cherche');
		conv.apply(feed.next({ type: 'agent_spawned', agent: spawnInfo({ agent_id: 'ag-1' }) }, { at: T0 + 100 }));

		expect(conv.agents.get('ag-1')).toMatchObject({ state: 'created', spawnedAt: T0 + 100 });
		expect(conv.agents.get('ag-1')?.spawn.profile).toBe('explore');

		conv.apply(feed.next({ type: 'agent_state', agent_id: 'ag-1', state: 'queued' }));
		expect(conv.agents.get('ag-1')?.state).toBe('queued');

		conv.apply(feed.next({ type: 'agent_state', agent_id: 'ag-1', state: 'running' }));
		expect(conv.agents.get('ag-1')?.state).toBe('running');
		expect(conv.agents.get('ag-1')?.endedAt).toBeUndefined();

		conv.apply(feed.next({ type: 'agent_state', agent_id: 'ag-1', state: 'completed' }, { at: T0 + 2_000 }));
		expect(conv.agents.get('ag-1')).toMatchObject({ state: 'completed', endedAt: T0 + 2_000 });
	});

	it('tracks the agent tool calls and keeps the result without overwriting its end time', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });

		startRun(conv, feed, 'Cherche');
		conv.apply(feed.next({ type: 'agent_spawned', agent: spawnInfo({ agent_id: 'ag-1' }) }));
		conv.apply(feed.next({ type: 'agent_state', agent_id: 'ag-1', state: 'completed' }, { at: T0 + 2_000 }));
		conv.apply(
			feed.next({ type: 'agent_tool_started', agent_id: 'ag-1', tool_call_id: 'atc-1', name: 'Grep', input: { pattern: 'x' } })
		);
		conv.apply(
			feed.next({ type: 'agent_tool_finished', agent_id: 'ag-1', tool_call_id: 'atc-1', name: 'Grep', is_error: false, duration_ms: 9 })
		);
		conv.apply(feed.next({ type: 'agent_finished', result: agentResult({ agent_id: 'ag-1' }) }, { at: T0 + 2_500 }));

		const agent = conv.agents.get('ag-1');
		expect(agent?.tools).toEqual([
			{ toolCallId: 'atc-1', name: 'Grep', input: { pattern: 'x' }, status: 'ok', durationMs: 9 }
		]);
		expect(agent?.result?.summary).toBe('Trois occurrences trouvées.');
		expect(agent?.endedAt).toBe(T0 + 2_000);
	});

	it('ignores state changes for an unknown agent', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });

		startRun(conv, feed, 'Cherche');
		conv.apply(feed.next({ type: 'agent_state', agent_id: 'ag-404', state: 'running' }));

		expect(conv.agents.size).toBe(0);
	});
});
