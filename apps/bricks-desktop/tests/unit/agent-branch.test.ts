// The branch of a sub-agent (features/agents/branch.ts): the steps its agent keeps in the parent
// conversation (AgentInfo.log, filled by Conversation.apply and hydrateAgents), replayed as a
// read-only conversation of its own. Hand-built envelopes, no rendering (see agent-branch-render).
import { describe, expect, it } from 'vitest';
import { Conversation, type AgentInfo, type TimelineItem } from '../../src/lib/state/conversation.svelte';
import { agentBranch, agentName, branchRunId, applyAgentSteps } from '../../src/lib/features/agents/branch';
import type { InstanceRecord, SpawnInfo } from '../../src/lib/ipc/engine';
import { EnvelopeFeed, MODEL, PROJECT_ID, T0, agentResult, openedSession, spawnInfo } from './fixtures';

const RUN = 'run-1';
const AGENT = 'ag-1';

/**
 * A parent conversation running one run that launched the sub-agent `ag-1` from call `tc-A`.
 * The parent's own run is started, so the sub-agent is placed like in the thread.
 */
function parentWithAgent(spawn: SpawnInfo = spawnInfo({ agent_id: AGENT, tool_call_id: 'tc-A' })) {
	const conv = new Conversation(openedSession(), PROJECT_ID);
	const feed = new EnvelopeFeed({ runId: RUN });
	conv.apply(feed.next({ type: 'run_started', prompt: 'Cherche', attachments: [], model: MODEL }, { at: T0 }));
	conv.apply(feed.next({ type: 'tool_started', tool_call_id: 'tc-A', name: 'Agent', input: {} }));
	conv.apply(feed.next({ type: 'agent_spawned', agent: spawn }, { at: T0 + 100 }));
	const agent = conv.agents.get(AGENT)!;
	return { conv, feed, agent };
}

/** The steps of the log as their event types, in order. */
function kinds(agent: AgentInfo): string[] {
	return agent.log.map((step) => step.event.type);
}

/** The timeline items of a branch as their kinds, in order. */
function itemKinds(items: readonly TimelineItem[]): string[] {
	return items.map((item) => item.kind);
}

describe('the log of a sub-agent', () => {
	it('starts with its mission, then fills in the order of its events, timed as they came', () => {
		const { conv, feed, agent } = parentWithAgent();
		expect(agent.log).toEqual([
			{ event: expect.objectContaining({ type: 'run_started', prompt: 'Chercher les occurrences' }), at: T0 + 100, execution: 1 },
		]);

		conv.apply(feed.next({ type: 'agent_tool_started', agent_id: AGENT, tool_call_id: 'tc-1', name: 'Read', input: { file_path: 'a.ts' } }, { at: T0 + 200 }));
		conv.apply(
			feed.next(
				{ type: 'agent_tool_finished', agent_id: AGENT, tool_call_id: 'tc-1', name: 'Read', is_error: false, duration_ms: 30, executed: true, status: 'success', output: 'const a = 1;', output_truncated: false },
				{ at: T0 + 230 },
			),
		);
		conv.apply(feed.next({ type: 'agent_text', agent_id: AGENT, text: 'Je regarde.' }, { at: T0 + 240 }));
		conv.apply(feed.next({ type: 'agent_finished', result: agentResult({ agent_id: AGENT }) }, { at: T0 + 300 }));

		expect(kinds(agent)).toEqual(['run_started', 'tool_started', 'tool_finished', 'text_delta', 'run_finished']);
		expect(agent.log.map((step) => step.at)).toEqual([T0 + 100, T0 + 200, T0 + 230, T0 + 240, T0 + 300]);
		expect(agent.log[2].event).toMatchObject({ type: 'tool_finished', name: 'Read', output: 'const a = 1;', is_error: false });
		expect(agent.log[3].event).toEqual({ type: 'text_delta', text: 'Je regarde.' });
		expect(agent.log[4].event).toMatchObject({ type: 'run_finished', outcome: 'succeeded', text: 'Trois occurrences trouvées.' });
	});

	it('gives the report as the answer when no text came before the end', () => {
		const { conv, feed, agent } = parentWithAgent();
		conv.apply(feed.next({ type: 'agent_tool_started', agent_id: AGENT, tool_call_id: 'tc-1', name: 'Grep', input: {} }));
		conv.apply(feed.next({ type: 'agent_finished', result: agentResult({ agent_id: AGENT, summary: 'Rapport seul.' }) }));

		expect(kinds(agent)).toEqual(['run_started', 'tool_started', 'text_delta', 'run_finished']);
		expect(agent.log[2].event).toEqual({ type: 'text_delta', text: 'Rapport seul.' });
	});

	it('does not repeat the report when the agent already wrote its answer', () => {
		const { conv, feed, agent } = parentWithAgent();
		conv.apply(feed.next({ type: 'agent_text', agent_id: AGENT, text: 'Réponse écrite.' }));
		conv.apply(feed.next({ type: 'agent_finished', result: agentResult({ agent_id: AGENT, summary: 'Rapport.' }) }));

		const texts = agent.log.filter((step) => step.event.type === 'text_delta').map((step) => step.event);
		expect(texts).toEqual([{ type: 'text_delta', text: 'Réponse écrite.' }]);
	});

	it.each([
		['completed', 'succeeded'],
		['failed', 'failed'],
		['cancelled', 'cancelled'],
		['interrupted', 'cancelled'],
		['incomplete', 'incomplete'],
	])('an agent that ends %s ends its run %s', (status, outcome) => {
		const { conv, feed, agent } = parentWithAgent();
		conv.apply(
			feed.next({ type: 'agent_finished', result: agentResult({ agent_id: AGENT, status, error: status === 'failed' ? 'boom' : undefined }) }),
		);

		const end = agent.log.at(-1)!.event;
		expect(end).toMatchObject({ type: 'run_finished', outcome });
	});

	it('a resume adds a run with its own mission, and an older execution changes nothing', () => {
		const { conv, feed, agent } = parentWithAgent();
		conv.apply(feed.next({ type: 'agent_finished', result: agentResult({ agent_id: AGENT, summary: 'Premier.' }) }, { at: T0 + 500 }));
		conv.apply(feed.next({ type: 'agent_state', agent_id: AGENT, state: 'running', execution: 2 }, { at: T0 + 900 }));

		expect(agent.log.at(-1)).toMatchObject({
			event: { type: 'run_started', prompt: 'Reprise de la mission' },
			at: T0 + 900,
			execution: 2,
		});

		const before = agent.log.length;
		conv.apply(feed.next({ type: 'agent_finished', result: agentResult({ agent_id: AGENT, execution: 1, summary: 'Ancien.' }) }, { at: T0 + 950 }));
		expect(agent.log).toHaveLength(before);
	});
});

describe('agentBranch: the branch is a conversation of its own', () => {
	it('reads like a conversation: the mission, the tool call with its output, the answer, the end of the run', () => {
		const { conv, feed, agent } = parentWithAgent();
		conv.apply(feed.next({ type: 'agent_tool_started', agent_id: AGENT, tool_call_id: 'tc-1', name: 'Read', input: { file_path: 'a.ts' } }));
		conv.apply(
			feed.next({ type: 'agent_tool_finished', agent_id: AGENT, tool_call_id: 'tc-1', name: 'Read', is_error: false, duration_ms: 30, executed: true, status: 'success', output: 'const a = 1;', output_truncated: false }),
		);
		conv.apply(feed.next({ type: 'agent_text', agent_id: AGENT, text: 'Voilà.' }));
		conv.apply(feed.next({ type: 'agent_finished', result: agentResult({ agent_id: AGENT }) }));

		const branch = agentBranch(conv, agent);

		expect(itemKinds(branch.items)).toEqual(['user', 'tool', 'assistant', 'run_end']);
		expect(branch.items[0]).toMatchObject({ kind: 'user', text: 'Chercher les occurrences', runId: branchRunId(AGENT, 1) });
		expect(branch.items[1]).toMatchObject({ kind: 'tool', name: 'Read', status: 'ok', output: 'const a = 1;' });
		expect(branch.items[2]).toMatchObject({ kind: 'assistant', text: 'Voilà.', streaming: false });
		expect(branch.items[3]).toMatchObject({ kind: 'run_end', runId: branchRunId(AGENT, 1) });
	});

	it('is read-only, titled after the agent, and leaves the parent conversation alone', () => {
		const { conv, agent } = parentWithAgent(spawnInfo({ agent_id: AGENT, tool_call_id: 'tc-A', description: 'Lire le code' }));
		const parentItems = conv.items.length;
		const parentLog = agent.log.length;

		const branch = agentBranch(conv, agent);

		expect(branch.readOnly).toBe(true);
		expect(conv.readOnly).toBe(false);
		expect(branch.sessionId).toBe(conv.sessionId);
		expect(branch.title).toBe('Lire le code');
		expect(conv.items).toHaveLength(parentItems);
		expect(agent.log).toHaveLength(parentLog);
	});

	it('does not show the parent conversation: its own prompt is not in the branch', () => {
		const { conv, feed, agent } = parentWithAgent();
		conv.apply(feed.next({ type: 'agent_finished', result: agentResult({ agent_id: AGENT }) }));

		const branch = agentBranch(conv, agent);
		expect(branch.items.some((item) => item.kind === 'user' && item.text === 'Cherche')).toBe(false);
	});

	it('is running from the start of its run until the end, and not before', () => {
		const { conv, feed, agent } = parentWithAgent();
		expect(agentBranch(conv, agent).running).toBe(true);
		expect(agentBranch(conv, agent).runningRunId).toBe(branchRunId(AGENT, 1));

		conv.apply(feed.next({ type: 'agent_tool_started', agent_id: AGENT, tool_call_id: 'tc-1', name: 'Read', input: {} }));
		expect(agentBranch(conv, agent).running).toBe(true);

		conv.apply(feed.next({ type: 'agent_finished', result: agentResult({ agent_id: AGENT }) }));
		const done = agentBranch(conv, agent);
		expect(done.running).toBe(false);
		expect(done.runningRunId).toBeNull();
	});

	it('a resumed agent has a second run in its branch, with the resumption as its mission', () => {
		const { conv, feed, agent } = parentWithAgent();
		conv.apply(feed.next({ type: 'agent_finished', result: agentResult({ agent_id: AGENT, summary: 'Premier.' }) }));
		conv.apply(feed.next({ type: 'agent_state', agent_id: AGENT, state: 'running', execution: 2 }));
		conv.apply(feed.next({ type: 'agent_text', agent_id: AGENT, text: 'Je reprends.' }));
		conv.apply(feed.next({ type: 'agent_finished', result: agentResult({ agent_id: AGENT, execution: 2, summary: 'Nouveau.' }) }));

		const branch = agentBranch(conv, agent);

		expect(branch.runs.size).toBe(2);
		expect(branch.runs.has(branchRunId(AGENT, 1))).toBe(true);
		expect(branch.runs.has(branchRunId(AGENT, 2))).toBe(true);
		const missions = branch.items.flatMap((item) => (item.kind === 'user' ? [item.text] : []));
		expect(missions).toEqual(['Chercher les occurrences', 'Reprise de la mission']);
		expect(itemKinds(branch.items)).toEqual(['user', 'assistant', 'run_end', 'user', 'assistant', 'run_end']);
		expect(branch.items.at(-2)).toMatchObject({ kind: 'assistant', text: 'Je reprends.', runId: branchRunId(AGENT, 2) });
	});
});

describe('agentName: the name of a sub-agent in the title bar', () => {
	it('is its description when it has one, else its profile', () => {
		expect(agentName({ spawn: spawnInfo({ description: 'Relire les tests' }) })).toBe('Relire les tests');
		expect(agentName({ spawn: spawnInfo({ description: '   ' }) })).toBe('Agent explore');
		expect(agentName({ spawn: spawnInfo({ profile: 'review', description: undefined }) })).toBe('Agent review');
	});
});

describe('hydrateAgents: a reopened session keeps readable branches', () => {
	function record(overrides: Partial<SpawnInfo> = {}, extra: Partial<InstanceRecord> = {}): InstanceRecord {
		return {
			info: spawnInfo(overrides),
			state: 'completed',
			background: false,
			updated_at_ms: T0 + 500,
			...extra,
		};
	}

	it('a restored agent with its result has a branch with its mission, its answer and the end of its run', () => {
		const conv = new Conversation(openedSession(), PROJECT_ID);
		conv.hydrateAgents(
			[record({ agent_id: AGENT }, { result: agentResult({ agent_id: AGENT, summary: 'Fait après reprise.' }) })],
			[],
		);
		const agent = conv.agents.get(AGENT)!;

		const branch = agentBranch(conv, agent);

		expect(itemKinds(branch.items)).toEqual(['user', 'assistant', 'run_end']);
		expect(branch.items[0]).toMatchObject({ kind: 'user', text: 'Chercher les occurrences' });
		expect(branch.items[1]).toMatchObject({ kind: 'assistant', text: 'Fait après reprise.' });
		expect(branch.running).toBe(false);
		expect(branch.readOnly).toBe(true);
	});

	it('a restored agent without a result has a mission and no answer yet', () => {
		const conv = new Conversation(openedSession(), PROJECT_ID);
		conv.hydrateAgents([record({ agent_id: AGENT }, { state: 'running' })], []);
		const agent = conv.agents.get(AGENT)!;

		const branch = agentBranch(conv, agent);

		expect(itemKinds(branch.items)).toEqual(['user']);
		expect(branch.running).toBe(true);
	});
});

describe('branchRunId and the first run of a branch', () => {
	it('names a run by its agent and its execution', () => {
		expect(branchRunId('ag-1', 1)).toBe('ag-1:1');
		expect(branchRunId('ag-1', 2)).toBe('ag-1:2');
	});
});

describe('a branch fed step by step', () => {
	it('keeps the items it has when new steps arrive: the thread is never re-keyed', () => {
		const { conv, agent } = parentWithAgent();
		const branch = agentBranch(conv, agent);
		const first = branch.items[0];
		const before = agent.log.length;
		agent.log.push({ event: { type: 'text_delta', text: 'Encore.' }, at: T0 + 900, execution: agent.execution });
		applyAgentSteps(branch, agent, before);
		expect(branch.items[0]).toBe(first);
	});

	it('closes a restored run opened last, so it never stays running', () => {
		const conv = new Conversation(openedSession(), PROJECT_ID);
		conv.hydrateAgents(
			[
				{
					info: spawnInfo({ agent_id: AGENT }),
					state: 'running',
					background: false,
					updated_at_ms: T0 + 500,
					execution: 2,
					result: agentResult({ agent_id: AGENT, execution: 1, summary: 'Ancien rapport.' }),
				},
			],
			[],
		);
		const branch = agentBranch(conv, conv.agents.get(AGENT)!);
		expect(branch.running).toBe(false);
	});
});
