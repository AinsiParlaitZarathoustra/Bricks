// The time a run worked, as the « bilan » shows it: MM:SS under an hour, HH:MM:SS from an hour.
import { describe, expect, it } from 'vitest';
import { formatWorked, runEndSummary } from '../../src/lib/features/conversation/runend';
import type { RunInfo } from '../../src/lib/state/conversation.svelte';

describe('formatWorked', () => {
	it('cuts the seconds, never rounds them', () => {
		expect(formatWorked(762)).toBe('00:00');
		expect(formatWorked(132_386)).toBe('02:12');
		expect(formatWorked(59_000)).toBe('00:59');
		expect(formatWorked(59_999)).toBe('00:59');
	});

	it('is MM:SS up to one hour excluded, and HH:MM:SS from one hour', () => {
		expect(formatWorked(60_000)).toBe('01:00');
		expect(formatWorked(3_599_000)).toBe('59:59');
		expect(formatWorked(3_600_000)).toBe('01:00:00');
		expect(formatWorked(3_723_000)).toBe('01:02:03');
	});

	it('goes past a day in hours', () => {
		expect(formatWorked(25 * 3_600_000 + 5_000)).toBe('25:00:05');
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
		expect(runEndSummary(run(1_000 + 132_386)).worked).toBe('02:12');
	});

	it('shows no worked time without the end, or with an end before the start', () => {
		expect(runEndSummary(run()).worked).toBeNull();
		expect(runEndSummary(run(500)).worked).toBeNull();
	});
});
