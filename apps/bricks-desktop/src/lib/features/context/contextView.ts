// Context occupation and session consumption as the desktop shows them: the
// composer ring and its panel, the context card and `/context` all read these.
// Pure functions only.
//
// Rules, in one place:
// - The numerator is the active occupation (`context_used.tokens`) whatever its
//   provenance (measured, counted, mixed or estimated). The provenance stays in
//   the engine data; the line shows the number alone.
// - The denominator is the input budget (`input_limit`, the model's
//   `max_input_tokens`). The output is a separate limit, never added to it. A
//   missing budget shows `—`.
// - A missing numerator shows `—`. A stale measure (model change, compaction,
//   clear) shows `—` for both figures until the next `context` event.
// - Cumulative billed totals are never a numerator.

import type { ContextChars, ContextStatus } from '$lib/ipc/engine';

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
	/** Input budget of the model, `null` when not reported or stale. */
	total: number | null;
}

function isCount(n: unknown): n is number {
	return typeof n === 'number' && Number.isFinite(n) && n >= 0;
}

export function contextFigures(status: ContextStatus | null, stale: boolean): ContextFigures {
	if (!status || stale) return { used: null, total: null };
	const used = status.context_used?.tokens;
	const total = status.input_limit;
	return {
		used: isCount(used) ? used : null,
		total: isCount(total) && total > 0 ? total : null
	};
}

/** The context line: `12 374 / 1 000 000 tokens`, with `—` for each missing figure. */
export function contextLine(status: ContextStatus | null, stale: boolean): string {
	const { used, total } = contextFigures(status, stale);
	const numerator = used === null ? '—' : formatNumber(used);
	const denominator = total === null ? '—' : formatNumber(total);
	return `${numerator} / ${denominator} tokens`;
}

/**
 * The composer ring. `ratio` is the drawn fill, bounded to 0–1; `percent` the
 * real share (it may exceed 100). Both are `null` when the occupation or the
 * input budget is unknown or stale: the ring is then neutral, without a figure.
 */
export interface ContextGauge {
	used: number | null;
	total: number | null;
	ratio: number | null;
	percent: number | null;
}

export function contextGauge(status: ContextStatus | null, stale: boolean): ContextGauge {
	const { used, total } = contextFigures(status, stale);
	if (used === null || total === null) return { used, total, ratio: null, percent: null };
	const share = used / total;
	return { used, total, ratio: Math.min(1, Math.max(0, share)), percent: share * 100 };
}

const COMPACT = new Intl.NumberFormat('fr-FR', { notation: 'compact', maximumFractionDigits: 1 });
const PERCENT = new Intl.NumberFormat('fr-FR', { maximumFractionDigits: 1, minimumFractionDigits: 1 });

/** Compact figure of the ring panel: `686,1 k`, `1 M`. */
export function formatCompact(n: number): string {
	return COMPACT.format(n);
}

export function formatPercent(p: number): string {
	return `${PERCENT.format(p)} %`;
}

/** The ring panel's value: `686,1 k / 1 M (65,5 %)`, `—` for each missing figure. */
export function gaugeLine(gauge: ContextGauge): string {
	const used = gauge.used === null ? '—' : formatCompact(gauge.used);
	const total = gauge.total === null ? '—' : formatCompact(gauge.total);
	const share = gauge.percent === null ? '' : ` (${formatPercent(gauge.percent)})`;
	return `${used} / ${total}${share}`;
}

/**
 * Share of prompt tokens read from the cache over the session, in percent:
 * cache read / (uncached input + cache read + cache write). Output tokens are
 * not prompt tokens. `null` (shown `—`) when no prompt token was counted.
 */
export function cacheHitRate(status: ContextStatus | null): number | null {
	const t = status?.totals;
	if (!t) return null;
	const prompt = t.input_tokens + t.cache_read_tokens + t.cache_write_tokens;
	return prompt > 0 ? (t.cache_read_tokens / prompt) * 100 : null;
}

/** Short figure of the context card: `812`, `33k`, `284.9k`, `1M`. */
export function formatShort(n: number): string {
	const v = Math.round(n);
	if (v < 1000) return String(v);
	// Rounded to one decimal first, so 999 950 reads `1M`, never `1000k`.
	const k = Math.round(v / 100) / 10;
	if (k < 1000) return `${k}k`;
	return `${Math.round(v / 100_000) / 10}M`;
}

export type ContextSource = keyof ContextChars;

/** One source of the context card: its share of the counted text, and the occupation that share stands for. */
export interface ContextShare {
	source: ContextSource;
	label: string;
	/** Part of the counted characters, 0–1. */
	share: number;
	/** The occupation times the share. */
	tokens: number;
}

const SOURCES: { source: ContextSource; label: string }[] = [
	{ source: 'conversation', label: 'Messages' },
	{ source: 'tool_results', label: 'Sorties d’outils' },
	{ source: 'tool_definitions', label: 'Définitions' },
	{ source: 'instructions', label: 'Prompt système' },
	{ source: 'other', label: 'Autre' }
];

/**
 * The sources of the occupation, from the last request's characters: each share is
 * its part of the counted text, its tokens the occupation times that share. Empty
 * sources are left out; `null` when the breakdown, the occupation or the text is
 * missing, or the figures are stale.
 */
export function contextShares(status: ContextStatus | null, stale: boolean): ContextShare[] | null {
	const chars = status?.breakdown?.chars;
	const { used } = contextFigures(status, stale);
	if (!chars || used === null) return null;
	let total = 0;
	for (const { source } of SOURCES) {
		const n = chars[source];
		if (!isCount(n)) return null;
		total += n;
	}
	if (total === 0) return null;
	return SOURCES.filter(({ source }) => chars[source] > 0).map(({ source, label }) => {
		const share = chars[source] / total;
		return { source, label, share, tokens: used * share };
	});
}
