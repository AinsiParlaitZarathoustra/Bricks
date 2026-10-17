// Display helpers of the inspector views (Sprint 14.1): French numbers,
// money and times, the cost reading rules, and readers for engine values
// that the typed IPC keeps loose (`unknown`). Pure functions only.
// Owner: Haiku 4 (Sprint 14.1).

import type { Usage } from '$lib/ipc/engine';

const INTEGER = new Intl.NumberFormat('fr-FR');
const USD = new Intl.NumberFormat('fr-FR', {
	style: 'currency',
	currency: 'USD',
	minimumFractionDigits: 2,
	maximumFractionDigits: 2
});
const USD_FINE = new Intl.NumberFormat('fr-FR', {
	style: 'currency',
	currency: 'USD',
	minimumFractionDigits: 4,
	maximumFractionDigits: 4
});
const CLOCK = new Intl.DateTimeFormat('fr-FR', {
	hour: '2-digit',
	minute: '2-digit',
	second: '2-digit'
});

export function formatInt(n: number): string {
	return INTEGER.format(Math.round(n));
}

/** `1 résultat`, `3 résultats`: the count and the right noun. */
export function plural(n: number, one: string, many: string): string {
	return `${formatInt(n)} ${n === 1 ? one : many}`;
}

export function formatTokens(n: number): string {
	return `${formatInt(n)} tokens`;
}

export function formatMs(ms: number): string {
	return `${formatInt(ms)} ms`;
}

export function formatClock(at: number): string {
	return CLOCK.format(new Date(at));
}

/** A percentage with a French space before the sign. */
export function formatPercent(p: number): string {
	if (p > 0 && p < 1) return '< 1 %';
	return `${Math.round(p)} %`;
}

/** A dollar amount: an exact zero reads as zero, a small positive amount never does. */
export function formatUsd(amount: number): string {
	if (amount === 0) return USD.format(0);
	if (amount > 0 && amount < 0.0001) return `< ${USD_FINE.format(0.0001)}`;
	return amount < 1 ? USD_FINE.format(amount) : USD.format(amount);
}

/** A text shortened for a label: one line, `…` at the end when cut. */
export function shortText(text: string, max: number): string {
	const oneLine = text.replace(/\s+/g, ' ').trim();
	return oneLine.length > max ? `${oneLine.slice(0, max - 1)}…` : oneLine;
}

/** Sum of the consumed token categories (input, output, cache read and write). */
export function consumedTokens(u: Usage | null | undefined): number {
	if (!u) return 0;
	return (
		u.input_tokens + u.output_tokens + u.cache_creation_input_tokens + u.cache_read_input_tokens
	);
}

/** What a view may say about the cost of some usage. */
export type CostReading =
	| { kind: 'unknown' }
	| { kind: 'exact'; usd: number }
	/** A lower bound: some consumed categories had no price. */
	| { kind: 'partial'; usd: number; unpriced: string[] };

/**
 * The cost of a usage, read as the TUI reads it: a price is shown only when
 * the engine gave one (`cost_usd`). Without it the cost is unknown, never
 * zero. A partial estimate (`cost_estimate.partial`) is a lower bound, not a total.
 */
export function costOf(usage: Usage | null | undefined): CostReading {
	const usd = usage?.cost_usd;
	if (typeof usd !== 'number' || !Number.isFinite(usd)) return { kind: 'unknown' };
	const estimate = usage?.cost_estimate;
	if (estimate?.partial) return { kind: 'partial', usd, unpriced: estimate.unpriced };
	return { kind: 'exact', usd };
}

/** Short cost text for a table cell. */
export function costCell(c: CostReading): string {
	switch (c.kind) {
		case 'unknown':
			return 'inconnu';
		case 'exact':
			return formatUsd(c.usd);
		case 'partial':
			return `≥ ${formatUsd(c.usd)} (partiel)`;
	}
}

/** Maintenance counters, read from the engine's report (`unknown` on the wire). */
export interface MaintenanceCounts {
	extracted: number | null;
	factsCreated: number | null;
	factsUpdated: number | null;
	embedded: number | null;
	failed: number | null;
	embeddingErrors: number | null;
	pending: number | null;
	cancelled: boolean | null;
	errors: string[];
}

export function readMaintenanceReport(v: unknown): MaintenanceCounts | null {
	if (typeof v !== 'object' || v === null) return null;
	const o = v as Record<string, unknown>;
	const num = (key: string): number | null => (typeof o[key] === 'number' ? (o[key] as number) : null);
	return {
		extracted: num('extracted'),
		factsCreated: num('facts_created'),
		factsUpdated: num('facts_updated'),
		embedded: num('embedded'),
		failed: num('failed'),
		embeddingErrors: num('embedding_errors'),
		pending: num('pending'),
		cancelled: typeof o.cancelled === 'boolean' ? o.cancelled : null,
		errors: Array.isArray(o.errors)
			? o.errors.filter((e): e is string => typeof e === 'string')
			: []
	};
}

/** Shows a count or a dash when the engine did not report it. */
export function countOrDash(n: number | null): string {
	return n === null ? '—' : formatInt(n);
}
