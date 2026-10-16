// Context occupation as the desktop shows it: one line, « ≈ 12 374 / 1 000 000 tokens ».
// Pure functions only. Owner: Haiku 1 (Sprint 14.2).
//
// Rules, in one place:
// - The numerator is the active occupation (`context_used.tokens`) whatever its
//   provenance (measured, counted, mixed or estimated). It is always qualified by `≈`.
// - The denominator is the model's total window (`context_window.total`). No
//   silent replacement by the input limit: a missing window shows `—`.
// - A missing numerator shows `—`. A stale measure (model change, compaction,
//   clear) shows `—` for both figures until the next `context` event.
// - Cumulative billed totals are never a numerator.

import type { ContextStatus } from '$lib/ipc/engine';

const NUMBER = new Intl.NumberFormat('fr-FR');
const CURRENCY = new Intl.NumberFormat('fr-FR', {
	style: 'currency',
	currency: 'USD',
	maximumFractionDigits: 4
});

export function formatNumber(n: number): string {
	return NUMBER.format(Math.round(n));
}

export function formatTokens(n: number): string {
	return `${formatNumber(n)} tokens`;
}

export function formatCost(usd: number): string {
	return CURRENCY.format(usd);
}

/** Figures of the context line; `null` is shown as `—`. */
export interface ContextFigures {
	/** Occupation of the next request, `null` when unknown or stale. */
	used: number | null;
	/** Total window of the model, `null` when not reported or stale. */
	total: number | null;
}

function isCount(n: unknown): n is number {
	return typeof n === 'number' && Number.isFinite(n) && n >= 0;
}

export function contextFigures(status: ContextStatus | null, stale: boolean): ContextFigures {
	if (!status || stale) return { used: null, total: null };
	const used = status.context_used?.tokens;
	const total = status.context_window?.total;
	return {
		used: isCount(used) ? used : null,
		total: isCount(total) && total > 0 ? total : null
	};
}

/** The context line: `≈ 12 374 / 1 000 000 tokens`, with `—` for each missing figure. */
export function contextLine(status: ContextStatus | null, stale: boolean): string {
	const { used, total } = contextFigures(status, stale);
	const numerator = used === null ? '—' : `≈ ${formatNumber(used)}`;
	const denominator = total === null ? '—' : formatNumber(total);
	return `${numerator} / ${denominator} tokens`;
}
