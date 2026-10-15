// Hand-built test fixtures for the frontend reducers. Test data only: nothing here
// is shipped in the product. Shapes follow src/lib/ipc/engine.ts and the generated
// bindings (src/lib/ipc/bindings).
import type { OpenedSession } from '../../src/lib/ipc/client';
import type {
	AgentResult,
	ContextStatus,
	Envelope,
	Event as EngineEvent,
	SpawnInfo,
	Snapshot,
	Usage
} from '../../src/lib/ipc/engine';

export const SESSION_ID = 'sess-test-1';
export const PROJECT_ID = 'proj-test-1';
export const MODEL = 'anthropic/claude-test';

/** Fixed epoch (ms) so timestamps are readable in assertions. */
export const T0 = 1_780_000_000_000;

export const ZERO_USAGE: Usage = {
	input_tokens: 0,
	output_tokens: 0,
	total_tokens: 0,
	cache_creation_input_tokens: 0,
	cache_read_input_tokens: 0,
	reasoning_tokens: 0
};

const CONTEXT: ContextStatus = {
	context_used: {
		tokens: 0,
		upper_bound: 0,
		measured_tokens: 0,
		estimated_tokens: 0,
		provenance: 'estimated',
		method: 'fixture'
	},
	context_window: { model: MODEL, total: 200_000, max_input_tokens: 180_000, max_output_tokens: 8_000 },
	input_limit: 180_000,
	reserved_output: 8_000,
	margin: 0,
	total_tokens: 0,
	totals: {
		input_tokens: 0,
		cache_read_tokens: 0,
		cache_write_tokens: 0,
		output_tokens: 0,
		reasoning_tokens: 0,
		requests: 0,
		requests_without_usage: 0,
		compaction_requests: 0
	},
	notes: []
};

export function snapshot(overrides: Partial<Snapshot> = {}): Snapshot {
	return {
		session_id: SESSION_ID,
		title: 'Essai',
		working_dir: '/work/demo',
		model: MODEL,
		reasoning: null,
		effective_reasoning: null,
		activity: { state: 'idle' },
		maintenance_running: false,
		interactive: true,
		context: CONTEXT,
		usage: ZERO_USAGE,
		pending_approvals: [],
		allowed_for_session: [],
		memory_space: null,
		approval_rules: null,
		...overrides
	};
}

/** A stored, idle session as `session_open` would return it. */
export function openedSession(overrides: Partial<OpenedSession> = {}): OpenedSession {
	return {
		session: {
			id: SESSION_ID,
			project_id: PROJECT_ID,
			title: 'Essai',
			created_at: T0,
			updated_at: T0,
			message_count: 0,
			model: MODEL,
			reasoning: null,
			working_dir: '/work/demo',
			open: true,
			running: false
		},
		snapshot: snapshot(),
		history: [],
		recent_events: [],
		last_seq: 0,
		...overrides
	};
}

/**
 * Builds envelopes with increasing `seq`, like the engine does. `at` defaults to
 * T0 + 10 ms per envelope; pass `at` to control durations explicitly.
 */
export class EnvelopeFeed {
	#seq = 0;
	readonly #runId: string | undefined;

	constructor(options: { runId?: string } = {}) {
		this.#runId = options.runId;
	}

	next(event: EngineEvent, options: { at?: number; runId?: string | null } = {}): Envelope {
		this.#seq += 1;
		const runId = options.runId === null ? undefined : (options.runId ?? this.#runId);
		return {
			...event,
			schema: 5,
			session_id: SESSION_ID,
			run_id: runId,
			seq: this.#seq,
			at: options.at ?? T0 + this.#seq * 10
		} as Envelope;
	}
}

export function spawnInfo(overrides: Partial<SpawnInfo> = {}): SpawnInfo {
	return {
		agent_id: 'ag-1',
		root_run_id: 'run-1',
		profile: 'explore',
		profile_source: 'builtin',
		profile_revision: 'r1',
		model: { requested: MODEL, applied: MODEL },
		reasoning: { requested: 'medium', applied: 'medium' },
		workspace: '/work/demo',
		isolation: 'none',
		task: 'Chercher les occurrences',
		created_at: new Date(T0).toISOString(),
		background: false,
		depth: 1,
		...overrides
	};
}

export function agentResult(overrides: Partial<AgentResult> = {}): AgentResult {
	return {
		agent_id: 'ag-1',
		profile: 'explore',
		status: 'completed',
		summary: 'Trois occurrences trouvées.',
		summary_truncated: false,
		files_changed: [],
		commands: [],
		warnings: [],
		turns: 2,
		tool_calls: 1,
		usage: ZERO_USAGE,
		duration_ms: 1200,
		model: MODEL,
		...overrides
	};
}
