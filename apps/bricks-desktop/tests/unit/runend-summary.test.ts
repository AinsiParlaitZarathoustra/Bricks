// The « bilan » line of a finished run: state, duration from the engine's timestamps,
// and edit totals only when the structured edit data allows them.
import { describe, expect, it } from 'vitest';
import type { FileTotals, RunInfo } from '../../src/lib/state/conversation.svelte';
import { runEndSummary } from '../../src/lib/features/conversation/runend';

const T0 = 1_780_000_000_000;

function run(overrides: Partial<RunInfo> = {}, files: FileTotals[] = []): RunInfo {
	const map = new Map(files.map((f) => [f.path, f]));
	return {
		runId: 'run-1',
		prompt: 'Teste',
		model: 'anthropic/claude-test',
		startedAt: T0,
		approvalsUnsatisfied: [],
		files: map as RunInfo['files'],
		commands: [],
		...overrides
	} as RunInfo;
}

const file = (path: string, added: number, removed: number, binary = false): FileTotals => ({
	key: `run-1|${path}`,
	path,
	absolute: null,
	name: path.split('/').pop() ?? path,
	added,
	removed,
	binary,
	kind: 'modify',
	operations: 1,
	agentIds: []
});

describe('runEndSummary: the state and the duration', () => {
	it('a succeeded run reads Terminé with its duration in compact units', () => {
		const s = runEndSummary(run({ outcome: 'succeeded', finishedAt: T0 + 762 }));
		expect(s).toMatchObject({ outcome: 'succeeded', label: 'Terminé', durationMs: 762 });
		expect(s.duration).toBe('762ms');
	});

	it('uses each state label and keeps Annulé, Échec and Incomplet', () => {
		expect(runEndSummary(run({ outcome: 'cancelled', finishedAt: T0 + 5 })).label).toBe('Annulé');
		expect(runEndSummary(run({ outcome: 'failed', finishedAt: T0 + 5 })).label).toBe('Échec');
		expect(runEndSummary(run({ outcome: 'incomplete', finishedAt: T0 + 5 })).label).toBe('Incomplet');
	});

	it('shows a neutral label while the outcome is unknown, with no duration before the finish', () => {
		expect(runEndSummary(run())).toMatchObject({ outcome: null, label: 'Fin de l’exécution', durationMs: null, duration: null });
	});

	it('shows no duration for an undefined run or a finish before the start', () => {
		expect(runEndSummary(undefined)).toMatchObject({ outcome: null, durationMs: null, additions: null });
		expect(runEndSummary(run({ outcome: 'succeeded', finishedAt: T0 - 1 })).durationMs).toBeNull();
	});

	it('keeps long durations in compact units', () => {
		const long = runEndSummary(run({ outcome: 'succeeded', finishedAt: T0 + 12_400 })).duration;
		expect(long?.replace(/\s/g, ' ')).toBe('12s');
	});
});

describe('runEndSummary: additions and deletions', () => {
	it('sums the structured edit operations of text files', () => {
		const s = runEndSummary(run({ outcome: 'succeeded', finishedAt: T0 + 1 }, [file('a.ts', 100, 10), file('b.ts', 20, 14)]));
		expect(s).toMatchObject({ additions: 120, deletions: 24 });
	});

	it('shows no counts when the run edited nothing', () => {
		expect(runEndSummary(run({ outcome: 'succeeded', finishedAt: T0 + 1 })).additions).toBeNull();
	});

	it('shows no counts when a binary file has no line count', () => {
		const s = runEndSummary(run({ outcome: 'succeeded', finishedAt: T0 + 1 }, [file('a.ts', 3, 1), file('logo.png', 0, 0, true)]));
		expect(s).toMatchObject({ additions: null, deletions: null });
	});
});

describe('runEndSummary: the reason and the error', () => {
	it('gives the reason of an incomplete run from its termination', () => {
		const s = runEndSummary(
			run({ outcome: 'incomplete', finishedAt: T0 + 1, termination: { kind: 'no_progress', repeats: 5 } as RunInfo['termination'] })
		);
		expect(s.reason).toBe('aucun progrès détecté (5 répétitions)');
	});

	it('reads a stored end of an earlier version as interrupted, never as a success', () => {
		// Historical data: a stop written before 0.4.8.
		const s = runEndSummary(
			run({ outcome: 'incomplete', finishedAt: T0 + 1, termination: { kind: 'max_turns', limit: 20 } as RunInfo['termination'] })
		);
		expect(s.reason).toBe('fin interrompue (ancienne session)');
	});

	it('gives the error of a failed run only', () => {
		expect(runEndSummary(run({ outcome: 'failed', finishedAt: T0 + 1, error: 'quota dépassé' })).error).toBe('quota dépassé');
		expect(runEndSummary(run({ outcome: 'cancelled', finishedAt: T0 + 1, error: 'ignorée' })).error).toBeNull();
	});
});
