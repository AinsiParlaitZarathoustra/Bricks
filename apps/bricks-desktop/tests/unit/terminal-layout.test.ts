// Terminal pane height as a share of the centre column (Sprint 14.3): bounds, pixel
// limits of both panes, proportional restore, and the one-time migration of the legacy
// `terminal_height` preference. Pure functions of src/lib/layout/geometry.ts.
import { describe, expect, it } from 'vitest';
import { LAYOUT_LIMITS, TERMINAL_RATIO } from '../../src/lib/layout/constants';
import {
	clampRatio,
	fitTerminalRatio,
	migrateTerminalRatio,
	ratioFromPx
} from '../../src/lib/layout/geometry';

describe('terminal ratio bounds', () => {
	it('keeps a saved ratio between 0.1 and 0.8', () => {
		expect(clampRatio(0.02)).toBe(TERMINAL_RATIO.min);
		expect(clampRatio(0.95)).toBe(TERMINAL_RATIO.max);
		expect(clampRatio(-3)).toBe(TERMINAL_RATIO.min);
		expect(clampRatio(0.1)).toBe(0.1);
		expect(clampRatio(0.8)).toBe(0.8);
	});

	it('rounds to three decimals so the saved value stays short', () => {
		expect(clampRatio(0.3333333)).toBe(0.333);
		expect(clampRatio(0.4567)).toBe(0.457);
	});

	it('gives the fallback for a value that is not a number', () => {
		expect(clampRatio(Number.NaN)).toBe(TERMINAL_RATIO.fallback);
		expect(clampRatio(Number.POSITIVE_INFINITY)).toBe(TERMINAL_RATIO.fallback);
	});
});

describe('fitTerminalRatio', () => {
	it('gives the share of the column in px', () => {
		expect(fitTerminalRatio(800, 0.3)).toBe(240);
		expect(fitTerminalRatio(800, 0.5)).toBe(400);
	});

	it('keeps the proportion when the window is resized', () => {
		expect(fitTerminalRatio(600, 0.3)).toBe(180);
		expect(fitTerminalRatio(1000, 0.3)).toBe(300);
	});

	it('never makes the terminal shorter than its minimum', () => {
		expect(fitTerminalRatio(800, 0.1)).toBe(LAYOUT_LIMITS.terminalMin);
		expect(fitTerminalRatio(300, 0.1)).toBe(LAYOUT_LIMITS.terminalMin);
	});

	it('leaves the conversation its minimum height when the column is tall enough', () => {
		const column = 1000;
		const px = fitTerminalRatio(column, 0.8);
		expect(column - px).toBeGreaterThanOrEqual(LAYOUT_LIMITS.conversationMin);
		expect(px).toBe(column - LAYOUT_LIMITS.conversationMin);
	});

	it('gives the terminal its minimum in a very small column, before the conversation', () => {
		expect(fitTerminalRatio(300, 0.8)).toBe(LAYOUT_LIMITS.terminalMin);
	});

	it('uses the clamped ratio for an out-of-range saved value', () => {
		expect(fitTerminalRatio(800, 5)).toBe(fitTerminalRatio(800, TERMINAL_RATIO.max));
		expect(fitTerminalRatio(800, -1)).toBe(fitTerminalRatio(800, TERMINAL_RATIO.min));
	});
});

describe('ratioFromPx (legacy terminal_height)', () => {
	it('takes the saved pixels against the current centre column', () => {
		expect(ratioFromPx(240, 800)).toBe(0.3);
		expect(ratioFromPx(400, 800)).toBe(0.5);
	});

	it('clamps a legacy height that is too small or too large for the bounds', () => {
		expect(ratioFromPx(50, 800)).toBe(TERMINAL_RATIO.min);
		expect(ratioFromPx(900, 800)).toBe(TERMINAL_RATIO.max);
	});

	it('gives the fallback until the column is measured', () => {
		expect(ratioFromPx(240, 0)).toBe(TERMINAL_RATIO.fallback);
	});
});

describe('migrateTerminalRatio', () => {
	it('derives the ratio once, from the legacy height, when none is saved', () => {
		expect(migrateTerminalRatio(null, 240, 800)).toBe(0.3);
	});

	it('keeps a saved ratio: the legacy height is ignored', () => {
		expect(migrateTerminalRatio(0.42, 240, 800)).toBeNull();
	});

	it('waits for the preferences: nothing to migrate while the legacy height is unknown', () => {
		expect(migrateTerminalRatio(null, undefined, 800)).toBeNull();
	});

	it('waits for the centre column to be measured', () => {
		expect(migrateTerminalRatio(null, 240, 0)).toBeNull();
	});

	it('migrates a legacy height outside the bounds to the nearest bound', () => {
		expect(migrateTerminalRatio(null, 10, 800)).toBe(TERMINAL_RATIO.min);
	});
});
