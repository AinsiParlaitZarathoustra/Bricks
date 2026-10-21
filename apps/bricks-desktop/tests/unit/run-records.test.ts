// What a run keeps of its commands and file changes, live and from its stored record:
// parent and sub-agent commands, refusals, failures, interruptions, background jobs, file
// keys, the time worked. Deterministic: envelopes built by hand, no clock.
import { describe, expect, it } from 'vitest';
import type { RunRecord } from '../../src/lib/ipc/client';
import type { Envelope, Event as EngineEvent } from '../../src/lib/ipc/engine';
import { Conversation, type CommandEvidence } from '../../src/lib/state/conversation.svelte';
import { EnvelopeFeed, PROJECT_ID, T0, agentResult, openedSession, snapshot, spawnInfo } from './fixtures';

const RUN_1 = 'run-1';

function openConversation(opened = openedSession()): Conversation {
	return new Conversation(opened, PROJECT_ID);
}

function startRun(conv: Conversation, feed: EnvelopeFeed, at = T0) {
	conv.apply(feed.next({ type: 'run_started', prompt: 'Lance', attachments: [], model: 'm' }, { runId: RUN_1, at }));
}

function endRun(conv: Conversation, feed: EnvelopeFeed, outcome: 'succeeded' | 'cancelled' = 'succeeded', at = T0 + 1_000) {
	conv.apply(feed.next({ type: 'run_finished', outcome, text: '', turns: 1 }, { runId: RUN_1, at }));
}

function bashStart(conv: Conversation, feed: EnvelopeFeed, id: string, command: string) {
	conv.apply(feed.next({ type: 'tool_started', tool_call_id: id, name: 'Bash', input: { command } }));
}

function bashEnd(conv: Conversation, feed: EnvelopeFeed, id: string, extra: Partial<Extract<EngineEvent, { type: 'tool_finished' }>> = {}) {
	conv.apply(
		feed.next({
			type: 'tool_finished',
			tool_call_id: id,
			name: 'Bash',
			is_error: false,
			duration_ms: 40,
			output: 'sortie',
			executed: true,
			status: 'success',
			...extra,
		}),
	);
}

function commands(conv: Conversation): CommandEvidence[] {
	return conv.run(RUN_1)?.commands ?? [];
}

function edit(conv: Conversation, feed: EnvelopeFeed, id: string, files: Extract<EngineEvent, { type: 'edit_applied' }>['files'], agentId?: string) {
	conv.apply(
		feed.next({
			type: 'edit_applied',
			tool_call_id: id,
			tool: 'Write',
			files,
			...(agentId ? { agent_id: agentId } : {}),
		}),
	);
}

describe('commands of a run', () => {
	it('the session agent and its sub-agents each have their own entries, under the run', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		startRun(conv, feed);
		bashStart(conv, feed, 'tc-1', 'ls');
		bashEnd(conv, feed, 'tc-1');
		conv.apply(feed.next({ type: 'notice', message: 'sub-agent ag-1 started' }));
		conv.apply(feed.next({ type: 'agent_spawned', agent: spawnInfo({ agent_id: 'ag-1', root_run_id: RUN_1 }) }));
		conv.apply(feed.next({ type: 'agent_tool_started', agent_id: 'ag-1', tool_call_id: 'tc-1', name: 'Bash', input: { command: 'cargo test' } }));
		conv.apply(feed.next({ type: 'agent_tool_finished', agent_id: 'ag-1', tool_call_id: 'tc-1', name: 'Bash', is_error: false, duration_ms: 3_000, executed: true, status: 'success', output: 'ok', output_truncated: false }));
		endRun(conv, feed);

		const list = commands(conv);
		expect(list.map((c) => c.key)).toEqual([`${RUN_1}||tc-1`, `${RUN_1}|ag-1|tc-1`]);
		expect(list[0]).toMatchObject({ agentId: null, command: 'ls', status: 'success', durationMs: 40, output: 'sortie' });
		expect(list[1]).toMatchObject({ agentId: 'ag-1', command: 'cargo test', status: 'success', durationMs: 3_000, output: 'ok' });
	});

	it('a refused call is not a command; a failed one is, with its failure', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		startRun(conv, feed);
		bashStart(conv, feed, 'tc-0', 'rm -rf x');
		bashEnd(conv, feed, 'tc-0', { is_error: true, executed: false, status: 'failure', output: 'Permission denied' });
		bashStart(conv, feed, 'tc-1', 'make');
		bashEnd(conv, feed, 'tc-1', { is_error: true, status: 'failure', output: 'erreur', duration_ms: 900 });
		endRun(conv, feed);

		expect(commands(conv)).toHaveLength(1);
		expect(commands(conv)[0]).toMatchObject({ toolCallId: 'tc-1', status: 'failure', isError: true, durationMs: 900 });
	});

	it('a shell call that never ended is an unknown command when the run ends, never a success', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		startRun(conv, feed);
		bashStart(conv, feed, 'tc-1', 'sleep 60');
		endRun(conv, feed, 'cancelled');
		expect(commands(conv)).toEqual([
			expect.objectContaining({ command: 'sleep 60', status: 'unknown', durationMs: null, output: null }),
		]);
	});

	it('a background command is running, then takes the end of its job', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		startRun(conv, feed);
		bashStart(conv, feed, 'tc-1', 'npm run dev');
		conv.apply(feed.next({ type: 'job_started', job_id: 'job_1', agent_id: 'root', root_run_id: RUN_1, command: 'npm run dev', cwd: '/w', pid: 7 }));
		bashEnd(conv, feed, 'tc-1', { status: 'running', output: 'démarré', duration_ms: 500 });
		expect(commands(conv)[0]).toMatchObject({ status: 'running', durationMs: 500 });
		endRun(conv, feed);
		expect(commands(conv)[0].status).toBe('running');

		conv.apply(feed.next({ type: 'job_finished', job_id: 'job_1', state: 'completed', code: 0, duration_ms: 5_000, logs: ['fin'] }));
		expect(commands(conv)[0]).toMatchObject({ status: 'success', durationMs: 5_000, output: 'fin' });
	});

	it('a job that ended before its command reported the handover takes that end when matched', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		startRun(conv, feed);
		bashStart(conv, feed, 'tc-1', 'build');
		conv.apply(feed.next({ type: 'job_started', job_id: 'job_2', agent_id: 'root', root_run_id: RUN_1, command: 'build', cwd: '/w', pid: 8 }));
		conv.apply(feed.next({ type: 'job_finished', job_id: 'job_2', state: 'failed', code: 2, duration_ms: 700, logs: [] }));
		bashEnd(conv, feed, 'tc-1', { status: 'running' });
		expect(commands(conv)[0]).toMatchObject({ status: 'failure', durationMs: 700 });
	});

	it('a job of a sub-agent is matched to that sub-agent\'s command, not the session agent\'s', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		startRun(conv, feed);
		conv.apply(feed.next({ type: 'agent_spawned', agent: spawnInfo({ agent_id: 'ag-1', root_run_id: RUN_1 }) }));
		conv.apply(feed.next({ type: 'agent_tool_started', agent_id: 'ag-1', tool_call_id: 'tc-9', name: 'Bash', input: { command: 'serve' } }));
		conv.apply(feed.next({ type: 'job_started', job_id: 'job_3', agent_id: 'ag-1', root_run_id: RUN_1, command: 'serve', cwd: '/w', pid: 9 }));
		conv.apply(feed.next({ type: 'agent_tool_finished', agent_id: 'ag-1', tool_call_id: 'tc-9', name: 'Bash', is_error: false, duration_ms: 10, executed: true, status: 'running', output: '', output_truncated: false }));
		conv.apply(feed.next({ type: 'job_finished', job_id: 'job_3', state: 'stopped', duration_ms: 99, logs: [] }));
		expect(commands(conv)).toEqual([expect.objectContaining({ agentId: 'ag-1', status: 'cancelled' })]);
	});

	it('the same tool call id of two agents is two commands', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		startRun(conv, feed);
		bashStart(conv, feed, 'tc-7', 'pwd');
		bashEnd(conv, feed, 'tc-7');
		conv.apply(feed.next({ type: 'agent_spawned', agent: spawnInfo({ agent_id: 'ag-2', root_run_id: RUN_1 }) }));
		conv.apply(feed.next({ type: 'agent_tool_started', agent_id: 'ag-2', tool_call_id: 'tc-7', name: 'Bash', input: { command: 'whoami' } }));
		conv.apply(feed.next({ type: 'agent_tool_finished', agent_id: 'ag-2', tool_call_id: 'tc-7', name: 'Bash', is_error: false, duration_ms: 1, executed: true, status: 'success', output: 'me', output_truncated: false }));
		endRun(conv, feed);
		expect(commands(conv).map((c) => [c.agentId, c.command])).toEqual([
			[null, 'pwd'],
			['ag-2', 'whoami'],
		]);
	});

	it('a non-shell tool is not a command', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		startRun(conv, feed);
		conv.apply(feed.next({ type: 'tool_started', tool_call_id: 'tc-j', name: 'BashTaskOutput', input: { job_id: 'job_1' } }));
		conv.apply(feed.next({ type: 'tool_finished', tool_call_id: 'tc-j', name: 'BashTaskOutput', is_error: false, duration_ms: 1, output: '', executed: true, status: 'success' }));
		endRun(conv, feed);
		expect(commands(conv)).toEqual([]);
	});
});

describe('files of a run', () => {
	it('edits add up per file, and a file is kept under its full path when it is known', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		startRun(conv, feed);
		const mod = (folder: string) => ({ path: `${folder}/mod.rs`, kind: 'modify' as const, added: 2, removed: 1, binary: false, absolute: `/work/demo/${folder}/mod.rs` });
		edit(conv, feed, 'tc-1', [mod('x'), mod('y')]);
		edit(conv, feed, 'tc-2', [mod('x')]);
		endRun(conv, feed);

		const files = [...(conv.run(RUN_1)?.files.values() ?? [])];
		expect(files.map((f) => [f.name, f.absolute, f.added, f.removed, f.operations])).toEqual([
			['mod.rs', '/work/demo/x/mod.rs', 4, 2, 2],
			['mod.rs', '/work/demo/y/mod.rs', 2, 1, 1],
		]);
		expect(conv.run(RUN_1)?.fileSummaries).toHaveLength(2);
	});

	it('a file created then deleted is last known as deleted, and a binary file has no count', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		startRun(conv, feed);
		edit(conv, feed, 'tc-1', [{ path: 'tmp.txt', kind: 'create', added: 3, removed: 0, binary: false }]);
		edit(conv, feed, 'tc-2', [{ path: 'tmp.txt', kind: 'delete', added: 0, removed: 3, binary: false }]);
		edit(conv, feed, 'tc-3', [{ path: 'logo.png', kind: 'modify', added: 0, removed: 0, binary: true }]);
		endRun(conv, feed);
		const run = conv.run(RUN_1)!;
		expect(run.files.get(`${RUN_1}|tmp.txt`)).toMatchObject({ kind: 'delete', added: 3, removed: 3, operations: 2 });
		expect(run.files.get(`${RUN_1}|logo.png`)).toMatchObject({ binary: true, added: 0, removed: 0 });
	});

	it('the same operation of a sub-agent is counted once, and each sub-agent is listed', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		startRun(conv, feed);
		conv.apply(feed.next({ type: 'agent_spawned', agent: spawnInfo({ agent_id: 'ag-1', root_run_id: RUN_1 }) }));
		const files = [{ path: 'a.ts', kind: 'modify' as const, added: 1, removed: 0, binary: false }];
		edit(conv, feed, 'tc-5', files, 'ag-1');
		edit(conv, feed, 'tc-5', files, 'ag-1');
		endRun(conv, feed);
		expect(conv.run(RUN_1)?.files.get(`${RUN_1}|a.ts`)).toMatchObject({ operations: 1, added: 1, agentIds: ['ag-1'] });
	});
});

describe('the time worked', () => {
	it('is the time between the engine timestamps of the run, and null without the end', () => {
		const conv = openConversation();
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		startRun(conv, feed, T0);
		endRun(conv, feed, 'succeeded', T0 + 3_723_000);
		expect(conv.run(RUN_1)?.workedMs).toBe(3_723_000);

		const open = openConversation();
		startRun(open, new EnvelopeFeed({ runId: RUN_1 }), T0);
		expect(open.run(RUN_1)?.workedMs).toBeNull();
	});
});

describe('stored runs', () => {
	const record = (overrides: Partial<RunRecord> = {}): RunRecord => ({
		version: 1,
		run_id: RUN_1,
		user_index: 0,
		started_at: T0,
		finished_at: T0 + 132_386,
		outcome: 'succeeded',
		files: [
			{ key: `${RUN_1}|/work/demo/a.ts`, path: 'a.ts', absolute: '/work/demo/a.ts', kind: 'modify', added: 4, removed: 1, binary: false, operations: 2, agent_ids: ['ag-1'] },
		],
		commands: [
			{ root_run_id: RUN_1, agent_id: null, tool_call_id: 'tc-1', name: 'Bash', command: 'npm test', status: 'failure', is_error: true, duration_ms: 9_000, output: 'échec', job_id: null },
		],
		assistant_text: 'Fait.',
		usage: null,
		...overrides,
	});
	const user = { role: 'user' as const, attachments: [], blocks: [{ type: 'text' as const, text: 'Lance' }] };

	it('the commands and files come from the record, and the time worked too', () => {
		const conv = openConversation(openedSession({ history: [user], run_records: [record()] }));
		const run = conv.run(RUN_1)!;
		expect(run.commands).toEqual([
			{
				key: `${RUN_1}||tc-1`,
				rootRunId: RUN_1,
				agentId: null,
				toolCallId: 'tc-1',
				name: 'Bash',
				command: 'npm test',
				status: 'failure',
				isError: true,
				durationMs: 9_000,
				output: 'échec',
			},
		]);
		expect([...run.files.values()]).toEqual([
			{ key: `${RUN_1}|/work/demo/a.ts`, path: 'a.ts', absolute: '/work/demo/a.ts', name: 'a.ts', kind: 'modify', added: 4, removed: 1, binary: false, operations: 2, agentIds: ['ag-1'] },
		]);
		expect(run.workedMs).toBe(132_386);
	});

	it('a record with an unknown end gives no time worked', () => {
		const conv = openConversation(openedSession({ history: [user], run_records: [record({ finished_at: null, outcome: null })] }));
		expect(conv.run(RUN_1)?.workedMs).toBeNull();
	});

	it('an envelope the opened view already holds is not applied a second time', () => {
		const feed = new EnvelopeFeed({ runId: RUN_1 });
		const started = feed.next({ type: 'run_started', prompt: 'Lance', attachments: [], model: 'm' }, { runId: RUN_1 });
		const text = feed.next({ type: 'text_delta', text: 'Déjà là' }, { runId: RUN_1 });
		// The run is still going: its events are replayed from the ring.
		const running = openedSession({
			snapshot: snapshot({ activity: { state: 'running', run_id: RUN_1 } }),
			recent_events: [started, text],
			last_seq: text.seq,
		});
		const conv = openConversation(running);
		expect(conv.items.filter((i) => i.kind === 'assistant').map((i) => (i as { text: string }).text)).toEqual([
			'Déjà là',
		]);
		// The same delivery again, from the channel, is ignored; a newer one is applied.
		conv.apply(text);
		conv.apply(feed.next({ type: 'text_delta', text: ' suite' }, { runId: RUN_1 }));
		expect(conv.items.filter((i) => i.kind === 'assistant').map((i) => (i as { text: string }).text)).toEqual([
			'Déjà là suite',
		]);
	});

	it('replaying the ring of a stored run does not count its commands or files again', () => {
		const feedEnvelopes = (): Envelope[] => {
			const feed = new EnvelopeFeed({ runId: RUN_1 });
			return [
				feed.next({ type: 'run_started', prompt: 'Lance', attachments: [], model: 'm' }, { runId: RUN_1 }),
				feed.next({ type: 'tool_started', tool_call_id: 'tc-1', name: 'Bash', input: { command: 'npm test' } }, { runId: RUN_1 }),
				feed.next({ type: 'tool_finished', tool_call_id: 'tc-1', name: 'Bash', is_error: true, duration_ms: 9_000, output: 'échec', executed: true, status: 'failure' }, { runId: RUN_1 }),
				feed.next({ type: 'edit_applied', tool_call_id: 'tc-2', tool: 'Edit', files: [{ path: 'a.ts', kind: 'modify', added: 4, removed: 1, binary: false, absolute: '/work/demo/a.ts' }] }, { runId: RUN_1 }),
				feed.next({ type: 'run_finished', outcome: 'succeeded', text: 'Fait.', turns: 1 }, { runId: RUN_1 }),
			];
		};
		const conv = openConversation(
			openedSession({ history: [user], run_records: [record()], recent_events: feedEnvelopes(), last_seq: 5 }),
		);
		expect(conv.run(RUN_1)?.commands).toHaveLength(1);
		expect(conv.run(RUN_1)?.files.get(`${RUN_1}|/work/demo/a.ts`)).toMatchObject({ added: 4, operations: 2 });
		expect(conv.items.filter((i) => i.kind === 'run_end')).toHaveLength(1);
	});
});
