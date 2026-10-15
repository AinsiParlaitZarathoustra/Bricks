// Context occupation as the ring and the detail card present it. Pure
// functions only: the rules that decide what may be shown as a percentage
// live here, in one place.

import type { ContextStatus, Provenance } from '$lib/ipc/engine';

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
	return `${formatNumber(n)} tokens`;
}

export function formatCost(usd: number): string {
	return CURRENCY.format(usd);
}

export const PROVENANCE_LABEL: Record<Provenance, string> = {
	measured: 'mesuré par le fournisseur',
	counted: 'compté par un point de comptage',
	mixed: 'mixte : base mesurée et ajout estimé',
	estimated: 'estimé localement'
};

export type ContextViewKind = 'empty' | 'stale' | 'unavailable' | 'measured' | 'overflow';
export type CapacityKind = 'window' | 'input_limit';

export interface ContextView {
	kind: ContextViewKind;
	/** Occupation in percent, not clamped. `null` unless measured or overflowing. */
	percent: number | null;
	/** Drawing fraction in [0, 1]. */
	fraction: number;
	/** Denominator used for the percentage, when known. */
	capacity: number | null;
	/** `window` only when the total window is reported; otherwise the input limit. */
	capacityKind: CapacityKind | null;
	/** Short text shown next to the ring. */
	headline: string;
	/** Accessible name of the ring. */
	ariaLabel: string;
	/** Rough figure, shown only when the exact one is not available. */
	estimate: number | null;
}

function formatPercent(p: number): string {
	if (p > 0 && p < 1) return '< 1 %';
	return `${Math.round(p)} %`;
}

/**
 * What the context ring may show.
 *
 * - A percentage only for `measured` or `counted` occupation, with a known
 *   capacity and no stale flag. Cumulative totals are never used here.
 * - Over 100 % is an overflow: flagged, drawn at 100 %.
 * - Otherwise the ring is blank, with `Indisponible`; `mixed` and `estimated`
 *   figures are offered as an estimate only.
 */
export function contextView(status: ContextStatus | null, stale: boolean): ContextView {
	if (!status) {
		return {
			kind: 'empty',
			percent: null,
			fraction: 0,
			capacity: null,
			capacityKind: null,
			headline: 'Indisponible',
			ariaLabel: 'Contexte : indisponible',
			estimate: null
		};
	}
	const total = status.context_window.total;
	const capacity = total && total > 0 ? total : status.input_limit > 0 ? status.input_limit : null;
	const capacityKind: CapacityKind | null =
		total && total > 0 ? 'window' : capacity !== null ? 'input_limit' : null;

	if (stale) {
		return {
			kind: 'stale',
			percent: null,
			fraction: 0,
			capacity,
			capacityKind,
			headline: 'Mise à jour au prochain tour',
			ariaLabel: 'Contexte : mise à jour au prochain tour',
			estimate: null
		};
	}

	const used = status.context_used;
	const exact = used.provenance === 'measured' || used.provenance === 'counted';
	if (!exact || capacity === null) {
		return {
			kind: 'unavailable',
			percent: null,
			fraction: 0,
			capacity,
			capacityKind,
			headline: 'Indisponible',
			ariaLabel: 'Contexte : indisponible',
			estimate: used.provenance === 'mixed' || used.provenance === 'estimated' ? used.tokens : null
		};
	}

	const percent = (used.tokens / capacity) * 100;
	const overflow = percent > 100;
	return {
		kind: overflow ? 'overflow' : 'measured',
		percent,
		fraction: Math.min(1, Math.max(0, percent / 100)),
		capacity,
		capacityKind,
		headline: overflow ? 'Dépassement' : formatPercent(percent),
		ariaLabel: overflow
			? `Contexte : dépassement, ${formatPercent(percent)} de la capacité`
			: `Contexte : ${formatPercent(percent)} utilisé`,
		estimate: null
	};
}
