// Unit tests for the.1 fields of the conversation reducer (Conversation.apply):
// usage, search, agent profiles, skills, memory, jobs, /clear, /compact, /skill, command
// refusals, and session isolation. Envelopes are hand-built with EnvelopeFeed (fixtures.ts).
import { describe, expect, it } from 'vitest';
import { Conversation } from '../../src/lib/state/conversation.svelte';
import type { Envelope, Usage } from '../../src/lib/ipc/engine';
import {
	EnvelopeFeed,
	PROJECT_ID,
	SESSION_ID,
	T0,
	ZERO_USAGE,
	openedSession,
	snapshot
} from './fixtures';

const SESSION_B = 'sess-test-2';
const RUN_A = 'run-a';

function conversation(sessionId: string = SESSION_ID, snapshotUsage?: Usage): Conversation {
	const base = openedSession();
	return new Conversation(
		openedSession({
			session: { ...base.session, id: sessionId },
			snapshot: snapshot({
				session_id: sessionId,
				...(snapshotUsage ? { usage: snapshotUsage } : {})
			})
		}),
		PROJECT_ID
	);
}

/** The same envelope, delivered for another session. */
function forSession(env: Envelope, sessionId: string): Envelope {
	return { ...env, session_id: sessionId };
}

function usage(input: number, output: number, extra: Partial<Usage> = {}): Usage {
	return {
		...ZERO_USAGE,
		input_tokens: input,
		output_tokens: output,
		total_tokens: input + output,
		...extra
	};
}

/** Fields of a conversation that.1 events can change, as plain values. */
function observable(conv: Conversation) {
	return {
		items: conv.items.length,
		turnUsages: conv.turnUsages.length,
		usageTotalFull: conv.usageTotalFull,
		usageTotal: conv.usageTotal,
		runUsages: conv.runUsages.size,
		jobs: conv.jobs.size,
		searchResult: conv.searchResult,
		agentProfiles: conv.agentProfiles,
		skillsListing: conv.skillsListing,
		skillsTrust: conv.skillsTrust,
		memory: conv.memory,
		lastClear: conv.lastClear,
		lastCompaction: conv.lastCompaction,
		lastSkill: conv.lastSkill,
		lastAgentControl: conv.lastAgentControl,
		lastRejection: conv.lastRejection,
		contextStale: conv.contextStale,
		running: conv.running,
		model: conv.model,
		lastSeq: conv.lastSeq
	};
}

describe('usage: one entry per response, the session total is replaced', () => {
	it('keeps each response in order, tagged with its run, and replaces the total', () => {
		const conv = conversation();
		const feed = new EnvelopeFeed({ runId: 'run-1' });

		conv.apply(
			feed.next({ type: 'usage', turn: usage(10, 5), total: usage(10, 5) }, { at: T0 + 100 })
		);
		conv.apply(
			feed.next({ type: 'usage', turn: usage(20, 8), total: usage(30, 13) }, { at: T0 + 200 })
		);

		expect(conv.turnUsages.map((t) => t.usage.input_tokens)).toEqual([10, 20]);
		expect(conv.turnUsages.map((t) => t.runId)).toEqual(['run-1', 'run-1']);
		expect(conv.turnUsages.map((t) => t.at)).toEqual([T0 + 100, T0 + 200]);
		// The engine's total is the session total: it is replaced, never added to.
		expect(conv.usageTotalFull?.input_tokens).toBe(30);
		expect(conv.usageTotalFull?.output_tokens).toBe(13);
		expect(conv.usageTotal).toEqual({ input: 30, output: 13, costUsd: undefined });
	});

	it('takes the session total from the snapshot before any response arrives', () => {
		const conv = conversation(SESSION_ID, usage(7, 3));
		expect(conv.usageTotalFull).toEqual(usage(7, 3));
		expect(conv.turnUsages).toEqual([]);
	});

	it('carries the priced cost of the total', () => {
		const conv = conversation();
		const feed = new EnvelopeFeed({ runId: 'run-1' });
		conv.apply(
			feed.next(
				{ type: 'usage', turn: usage(1, 1), total: usage(1, 1, { cost_usd: 0.42 }) },
				{ at: T0 + 10 }
			)
		);
		expect(conv.usageTotal).toEqual({ input: 1, output: 1, costUsd: 0.42 });
	});

	it('keeps an unpriced total unknown: no cost is invented', () => {
		const conv = conversation();
		const feed = new EnvelopeFeed({ runId: 'run-1' });
		conv.apply(feed.next({ type: 'usage', turn: usage(4, 2), total: usage(4, 2) }, { at: T0 + 10 }));
		expect(conv.usageTotal?.costUsd).toBeUndefined();
		expect(conv.usageTotalFull?.cost_usd).toBeUndefined();
	});
});

describe('run_usage: one answer per root run', () => {
	it('keeps the latest answer per root run, separately for each root', () => {
		const conv = conversation();
		const feed = new EnvelopeFeed({ runId: RUN_A });

		conv.apply(
			feed.next(
				{
					type: 'run_usage',
					root_run_id: RUN_A,
					own: usage(5, 1),
					descendants: usage(0, 0),
					total: usage(5, 1),
					pending_agents: 1,
					final_total: false
				},
				{ at: T0 + 10 }
			)
		);
		conv.apply(
			feed.next(
				{
					type: 'run_usage',
					root_run_id: RUN_A,
					own: usage(5, 1),
					descendants: usage(9, 4),
					total: usage(14, 5),
					pending_agents: 0,
					final_total: true
				},
				{ at: T0 + 20 }
			)
		);
		conv.apply(
			feed.next(
				{
					type: 'run_usage',
					root_run_id: 'run-b',
					own: usage(2, 2),
					descendants: usage(0, 0),
					total: usage(2, 2),
					pending_agents: 0,
					final_total: true
				},
				{ at: T0 + 30 }
			)
		);

		expect(conv.runUsages.size).toBe(2);
		const a = conv.runUsages.get(RUN_A);
		expect(a?.total.input_tokens).toBe(14);
		expect(a?.final_total).toBe(true);
		expect(a?.pending_agents).toBe(0);
		expect(conv.runUsages.get('run-b')?.total.input_tokens).toBe(2);
	});
});

describe('search_results: the workspace search of /search', () => {
	it('fills the result, with no notes when the engine sends none', () => {
		const conv = conversation();
		const feed = new EnvelopeFeed({ runId: RUN_A });

		conv.apply(
			feed.next(
				{
					type: 'search_results',
					query: 'TODO',
					status: 'ok',
					hits: [{ path: 'src/a.ts', line: 3, column: 1, text: 'TODO: revoir' }],
					omitted: 2,
					elapsed_ms: 14
				},
				{ at: T0 + 50 }
			)
		);

		expect(conv.searchResult).toEqual({
			at: T0 + 50,
			query: 'TODO',
			status: 'ok',
			hits: [{ path: 'src/a.ts', line: 3, column: 1, text: 'TODO: revoir' }],
			omitted: 2,
			notes: [],
			elapsedMs: 14
		});
	});

	it('keeps the engine notes and replaces the previous search', () => {
		const conv = conversation();
		const feed = new EnvelopeFeed({ runId: RUN_A });

		conv.apply(
			feed.next(
				{
					type: 'search_results',
					query: 'a(',
					status: 'error',
					hits: [],
					omitted: 0,
					notes: ['expression régulière invalide'],
					elapsed_ms: 1
				},
				{ at: T0 + 10 }
			)
		);
		conv.apply(
			feed.next(
				{ type: 'search_results', query: 'fn', status: 'ok', hits: [], omitted: 0, elapsed_ms: 2 },
				{ at: T0 + 20 }
			)
		);

		expect(conv.searchResult?.query).toBe('fn');
		expect(conv.searchResult?.notes).toEqual([]);
	});
});

describe('agent_profiles and skills: catalogues', () => {
	it('keeps the profile page and its diagnostics', () => {
		const conv = conversation();
		const feed = new EnvelopeFeed({ runId: RUN_A });

		conv.apply(
			feed.next({
				type: 'agent_profiles',
				profiles: [
					{
						name: 'explore',
						description: 'Explorer le code',
						scope: 'built_in',
						source: 'builtin',
						valid: true
					}
				],
				total: 1,
				page: 0,
				diagnostics: [{ source: 'project', message: 'profil ignoré' }]
			})
		);

		expect(conv.agentProfiles?.profiles.map((p) => p.name)).toEqual(['explore']);
		expect(conv.agentProfiles?.total).toBe(1);
		expect(conv.agentProfiles?.diagnostics).toEqual([{ source: 'project', message: 'profil ignoré' }]);
	});

	it('keeps the skills listing with its trust decision and diagnostics', () => {
		const conv = conversation();
		const feed = new EnvelopeFeed({ runId: RUN_A });

		conv.apply(
			feed.next(
				{
					type: 'skills',
					listing: {
						generation: 1,
						project_decision: 'pending',
						entries: [{ name: 'pdf', source: 'project', state: 'untrusted' }],
						total: 1,
						page: 0,
						diagnostics: ['SKILL.md sans description'],
						catalog_omitted: 0
					}
				},
				{ at: T0 + 40 }
			)
		);

		expect(conv.skillsListing?.at).toBe(T0 + 40);
		expect(conv.skillsListing?.listing.project_decision).toBe('pending');
		expect(conv.skillsListing?.listing.entries[0]).toMatchObject({ name: 'pdf', state: 'untrusted' });
		expect(conv.skillsListing?.listing.diagnostics).toEqual(['SKILL.md sans description']);
	});

	it('shows the trust request until it is resolved', () => {
		const conv = conversation();
		const feed = new EnvelopeFeed({ runId: RUN_A });

		conv.apply(
			feed.next({
				type: 'skills_trust_requested',
				project: '/work/demo',
				sources: [{ source: 'project', path: '/work/demo/.bricks/skills', skills: 2 }]
			})
		);
		expect(conv.skillsTrust?.project).toBe('/work/demo');
		expect(conv.skillsTrust?.sources[0].skills).toBe(2);

		conv.apply(
			feed.next({ type: 'skills_trust_resolved', project: '/work/demo', allowed: false, by: 'user' })
		);
		expect(conv.skillsTrust).toBeNull();
	});
});

describe('memory: recall and maintenance', () => {
	it('keeps the last recall: items, tokens, omitted and budget', () => {
		const conv = conversation();
		const feed = new EnvelopeFeed({ runId: RUN_A });

		conv.apply(
			feed.next(
				{ type: 'memory_recalled', items: 3, tokens: 412, omitted: 1, budget: 800 },
				{ at: T0 + 60 }
			)
		);
		expect(conv.memory.lastRecall).toEqual({
			at: T0 + 60,
			items: 3,
			tokens: 412,
			omitted: 1,
			budget: 800
		});
	});

	it('shows maintenance as running from its start to its end, then keeps the outcome', () => {
		const conv = conversation();
		const feed = new EnvelopeFeed({ runId: RUN_A });

		conv.apply(feed.next({ type: 'memory_maintenance_started' }, { at: T0 + 70 }));
		expect(conv.memory.maintenanceRunning).toBe(true);

		conv.apply(
			feed.next(
				{ type: 'memory_maintenance_finished', outcome: 'failed', error: 'disque plein' },
				{ at: T0 + 80 }
			)
		);
		expect(conv.memory.maintenanceRunning).toBe(false);
		expect(conv.memory.lastMaintenance).toEqual({
			at: T0 + 80,
			outcome: 'failed',
			report: undefined,
			error: 'disque plein'
		});
	});

	it('a cancelled maintenance is not shown as running', () => {
		const conv = conversation();
		const feed = new EnvelopeFeed({ runId: RUN_A });

		conv.apply(feed.next({ type: 'memory_maintenance_started' }, { at: T0 + 10 }));
		conv.apply(
			feed.next({ type: 'memory_maintenance_finished', outcome: 'cancelled' }, { at: T0 + 20 })
		);
		expect(conv.memory.maintenanceRunning).toBe(false);
		expect(conv.memory.lastMaintenance?.outcome).toBe('cancelled');
	});
});

describe('jobs: engine background jobs of the session', () => {
	it('registers a started job as running with no output yet', () => {
		const conv = conversation();
		const feed = new EnvelopeFeed({ runId: RUN_A });

		conv.apply(
			feed.next(
				{
					type: 'job_started',
					job_id: 'job-7',
					agent_id: 'ag-1',
					root_run_id: RUN_A,
					command: 'npm test',
					cwd: '/work/demo',
					pid: 4242
				},
				{ at: T0 + 90 }
			)
		);

		expect(conv.jobs.get('job-7')).toMatchObject({
			jobId: 'job-7',
			agentId: 'ag-1',
			rootRunId: RUN_A,
			command: 'npm test',
			cwd: '/work/demo',
			pid: 4242,
			startedAt: T0 + 90,
			stdoutBytes: 0,
			stderrBytes: 0,
			state: 'running',
			logs: []
		});
	});

	it('updates the byte counts while the job runs', () => {
		const conv = conversation();
		const feed = new EnvelopeFeed({ runId: RUN_A });
		conv.apply(
			feed.next({
				type: 'job_started',
				job_id: 'job-7',
				agent_id: 'ag-1',
				root_run_id: RUN_A,
				command: 'npm test',
				cwd: '/work/demo',
				pid: 1
			})
		);

		conv.apply(
			feed.next({ type: 'job_output', job_id: 'job-7', stdout_bytes: 120, stderr_bytes: 9 })
		);

		const job = conv.jobs.get('job-7');
		expect(job?.stdoutBytes).toBe(120);
		expect(job?.stderrBytes).toBe(9);
		expect(job?.state).toBe('running');
	});

	it('records how a job ended: state, exit code or signal, duration and logs', () => {
		const conv = conversation();
		const feed = new EnvelopeFeed({ runId: RUN_A });
		conv.apply(
			feed.next({
				type: 'job_started',
				job_id: 'job-7',
				agent_id: 'ag-1',
				root_run_id: RUN_A,
				command: 'sleep 100',
				cwd: '/work/demo',
				pid: 9
			})
		);

		conv.apply(
			feed.next({
				type: 'job_finished',
				job_id: 'job-7',
				state: 'stopped',
				signal: 15,
				duration_ms: 830,
				logs: ['arrêt demandé']
			})
		);

		expect(conv.jobs.get('job-7')).toMatchObject({
			state: 'stopped',
			signal: 15,
			code: undefined,
			durationMs: 830,
			logs: ['arrêt demandé']
		});
	});

	it('a finished job without logs keeps an empty log list', () => {
		const conv = conversation();
		const feed = new EnvelopeFeed({ runId: RUN_A });
		conv.apply(
			feed.next({
				type: 'job_started',
				job_id: 'job-8',
				agent_id: 'ag-1',
				root_run_id: RUN_A,
				command: 'true',
				cwd: '/work/demo',
				pid: 3
			})
		);
		conv.apply(feed.next({ type: 'job_finished', job_id: 'job-8', state: 'completed', code: 0, duration_ms: 5 }));
		expect(conv.jobs.get('job-8')).toMatchObject({ state: 'completed', code: 0, logs: [] });
	});

	it('job events for an unknown job are ignored', () => {
		const conv = conversation();
		const feed = new EnvelopeFeed({ runId: RUN_A });
		conv.apply(feed.next({ type: 'job_output', job_id: 'job-x', stdout_bytes: 1, stderr_bytes: 0 }));
		conv.apply(
			feed.next({ type: 'job_finished', job_id: 'job-x', state: 'failed', duration_ms: 1 })
		);
		expect(conv.jobs.size).toBe(0);
	});
});

describe('/clear, /compact, /skill and command answers', () => {
	it('context_cleared keeps the removed count, marks the context stale and keeps the conversation', () => {
		const conv = conversation();
		const feed = new EnvelopeFeed({ runId: RUN_A });
		conv.apply(
			feed.next({
				type: 'run_started',
				prompt: 'Bonjour',
				attachments: [],
				model: 'anthropic/claude-test'
			})
		);
		const before = conv.items.length;

		conv.apply(feed.next({ type: 'context_cleared', messages_removed: 4 }, { at: T0 + 300 }));

		expect(conv.lastClear).toEqual({ at: T0 + 300, messagesRemoved: 4 });
		expect(conv.contextStale).toBe(true);
		expect(conv.items.length).toBe(before + 1);
		expect(conv.items.at(-1)).toMatchObject({
			kind: 'notice',
			level: 'info',
			message: 'Contexte vidé (4 messages retirés).'
		});
	});

	it('compaction records its outcome; a compacted context is stale', () => {
		const conv = conversation();
		const feed = new EnvelopeFeed({ runId: RUN_A });

		conv.apply(
			feed.next(
				{ type: 'compaction', reason: 'manual', outcome: 'ok', compacted: true },
				{ at: T0 + 400 }
			)
		);

		expect(conv.lastCompaction).toEqual({
			at: T0 + 400,
			reason: 'manual',
			outcome: 'ok',
			compacted: true
		});
		expect(conv.contextStale).toBe(true);
		expect(conv.items.at(-1)).toMatchObject({
			kind: 'notice',
			message: 'Compaction (manual) : ok'
		});
	});

	it('a compaction that compacted nothing leaves the context fresh', () => {
		const conv = conversation();
		const feed = new EnvelopeFeed({ runId: RUN_A });
		conv.apply(
			feed.next({ type: 'compaction', reason: 'manual', outcome: 'nothing to do', compacted: false })
		);
		expect(conv.lastCompaction?.compacted).toBe(false);
		expect(conv.contextStale).toBe(false);
	});

	it('skill_activated becomes the last activation of the session', () => {
		const conv = conversation();
		const feed = new EnvelopeFeed({ runId: RUN_A });
		conv.apply(
			feed.next(
				{ type: 'skill_activated', name: 'pdf', source: 'project', revision: 'r1', origin: 'slash' },
				{ at: T0 + 500 }
			)
		);
		expect(conv.lastSkill).toEqual({ at: T0 + 500, name: 'pdf', ok: true });
	});

	it('a sub-agent skill activation does not replace the session activation', () => {
		const conv = conversation();
		const feed = new EnvelopeFeed({ runId: RUN_A });
		conv.apply(
			feed.next({
				type: 'skill_activated',
				agent_id: 'ag-1',
				name: 'pdf',
				source: 'project',
				revision: 'r1',
				origin: 'agent'
			})
		);
		expect(conv.lastSkill).toBeNull();
	});

	it('skill_activation_failed keeps the reason and shows it as an error notice', () => {
		const conv = conversation();
		const feed = new EnvelopeFeed({ runId: RUN_A });
		conv.apply(
			feed.next(
				{ type: 'skill_activation_failed', name: 'pdf', reason: 'source non approuvée' },
				{ at: T0 + 510 }
			)
		);
		expect(conv.lastSkill).toEqual({
			at: T0 + 510,
			name: 'pdf',
			ok: false,
			reason: 'source non approuvée'
		});
		expect(conv.items.at(-1)).toMatchObject({
			kind: 'notice',
			level: 'error',
			message: 'Skill « pdf » non activé : source non approuvée'
		});
	});

	it('a failed sub-agent activation is noticed but does not replace the session activation', () => {
		const conv = conversation();
		const feed = new EnvelopeFeed({ runId: RUN_A });
		conv.apply(
			feed.next({
				type: 'skill_activation_failed',
				agent_id: 'ag-1',
				name: 'pdf',
				reason: 'introuvable'
			})
		);
		expect(conv.lastSkill).toBeNull();
		expect(conv.items.at(-1)).toMatchObject({ kind: 'notice', level: 'error' });
	});

	it('agent_control_result keeps the last answer of an instance or changeset command', () => {
		const conv = conversation();
		const feed = new EnvelopeFeed({ runId: RUN_A });
		conv.apply(
			feed.next(
				{ type: 'agent_control_result', action: 'apply_changes', ok: false, text: 'conflit : rien écrit' },
				{ at: T0 + 600 }
			)
		);
		expect(conv.lastAgentControl).toEqual({
			action: 'apply_changes',
			ok: false,
			text: 'conflit : rien écrit',
			at: T0 + 600
		});
	});

	it('command_rejected keeps the refusal until the next run starts', () => {
		const conv = conversation();
		const feed = new EnvelopeFeed({ runId: RUN_A });
		conv.apply(
			feed.next(
				{ type: 'command_rejected', command: 'compact', reason: 'une exécution est en cours' },
				{ at: T0 + 700 }
			)
		);
		expect(conv.lastRejection).toEqual({
			command: 'compact',
			reason: 'une exécution est en cours',
			at: T0 + 700
		});

		conv.apply(
			feed.next({
				type: 'run_started',
				prompt: 'Suite',
				attachments: [],
				model: 'anthropic/claude-test'
			})
		);
		expect(conv.lastRejection).toBeNull();
	});
});

describe('session isolation: one Conversation per session, routed by session_id', () => {
	it('a stream for session A leaves session B unchanged', () => {
		const a = conversation(SESSION_ID);
		const b = conversation(SESSION_B);
		const before = observable(b);

		const feedA = new EnvelopeFeed({ runId: RUN_A });
		a.apply(
			feedA.next({ type: 'run_started', prompt: 'Analyse', attachments: [], model: 'anthropic/claude-test' })
		);
		a.apply(feedA.next({ type: 'usage', turn: usage(9, 3), total: usage(9, 3) }));
		a.apply(
			feedA.next({
				type: 'job_started',
				job_id: 'job-a',
				agent_id: 'ag-1',
				root_run_id: RUN_A,
				command: 'make',
				cwd: '/work/demo',
				pid: 2
			})
		);
		a.apply(feedA.next({ type: 'memory_recalled', items: 1, tokens: 10, omitted: 0, budget: 100 }));
		a.apply(feedA.next({ type: 'context_cleared', messages_removed: 2 }));
		a.apply(
			feedA.next({
				type: 'skill_activated',
				name: 'pdf',
				source: 'project',
				revision: 'r1',
				origin: 'slash'
			})
		);
		a.apply(feedA.next({ type: 'search_results', query: 'x', status: 'ok', hits: [], omitted: 0, elapsed_ms: 1 }));
		a.apply(
			feedA.next({ type: 'run_finished', outcome: 'succeeded', text: 'Fait', turns: 1 })
		);

		// The stream really changed A (the check below is not vacuous)...
		expect(a.turnUsages.length).toBe(1);
		expect(a.jobs.size).toBe(1);
		expect(a.lastClear?.messagesRemoved).toBe(2);
		// ...and B is exactly as it was.
		expect(observable(b)).toEqual(before);
	});

	it('each session keeps its own sequence numbers: a low seq in B is not dropped by A', () => {
		const a = conversation(SESSION_ID);
		const b = conversation(SESSION_B);

		const feedA = new EnvelopeFeed({ runId: RUN_A });
		for (let i = 0; i < 5; i++) {
			a.apply(feedA.next({ type: 'usage', turn: usage(i, i), total: usage(i, i) }));
		}
		expect(a.lastSeq).toBe(5);

		// B's first envelope has seq 1: it must be applied, not treated as a duplicate of A's.
		const feedB = new EnvelopeFeed({ runId: 'run-b' });
		b.apply(forSession(feedB.next({ type: 'usage', turn: usage(1, 1), total: usage(1, 1) }), SESSION_B));

		expect(b.turnUsages.length).toBe(1);
		expect(b.lastSeq).toBe(1);
	});

	// Found by Haiku 8: `Conversation.apply` now ignores envelopes of another session.
	it('a conversation ignores an envelope that belongs to another session', () => {
		const b = conversation(SESSION_B);
		const feedA = new EnvelopeFeed({ runId: RUN_A });
		b.apply(feedA.next({ type: 'usage', turn: usage(3, 1), total: usage(3, 1) }));
		expect(b.turnUsages.length).toBe(0);
	});
});
