// The time a run worked, as the « bilan » shows it: compact units.
import { describe, expect, it } from 'vitest';
import { formatWorked, runEndSummary } from '../../src/lib/features/conversation/runend';
import type { RunInfo } from '../../src/lib/state/conversation.svelte';

describe('formatWorked', () => {
	it('keeps milliseconds below a second and truncates longer durations', () => {
		expect(formatWorked(762)).toBe('762ms');
		expect(formatWorked(132_386)).toBe('2m 12s');
		expect(formatWorked(59_000)).toBe('59s');
		expect(formatWorked(59_999)).toBe('59s');
	});

	it('uses seconds, minutes and hours', () => {
		expect(formatWorked(60_000)).toBe('1m');
		expect(formatWorked(3_599_000)).toBe('59m 59s');
		expect(formatWorked(3_600_000)).toBe('1h');
		expect(formatWorked(3_723_000)).toBe('1h 2m 3s');
	});

	it('goes past a day in hours', () => {
		expect(formatWorked(25 * 3_600_000 + 5_000)).toBe('25h 5s');
	});

	it('is null when the time is absent, negative or not a number', () => {
		expect(formatWorked(null)).toBeNull();
		expect(formatWorked(-1)).toBeNull();
		expect(formatWorked(Number.NaN)).toBeNull();
	});
});

describe('runEndSummary carries the time worked', () => {
	const run = (finishedAt?: number): RunInfo =>
		({
			runId: 'run-1',
			prompt: 'p',
			model: 'm',
			startedAt: 1_000,
			finishedAt,
			outcome: 'succeeded',
			approvalsUnsatisfied: [],
			files: new Map(),
			fileSummaries: [],
			commands: [],
			workedMs: finishedAt === undefined ? null : finishedAt - 1_000,
		}) as unknown as RunInfo;

	it('shows the worked time from the engine timestamps', () => {
		expect(runEndSummary(run(1_000 + 132_386)).worked).toBe('2m 12s');
	});

	it('shows no worked time without the end, or with an end before the start', () => {
		expect(runEndSummary(run()).worked).toBeNull();
		expect(runEndSummary(run(500)).worked).toBeNull();
	});
});
