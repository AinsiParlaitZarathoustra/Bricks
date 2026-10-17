// Workspace text search (`/search`): reading of the input, grouping of the
// hits by file, highlighting of the query in a hit line, and the labels of
// the engine's status. Pure functions only. Owner: Haiku 4 (Sprint 14.1).
//
// The engine matches case-sensitively. A literal query is a plain string; a
// regular expression is compiled here as a JavaScript RegExp for the
// highlight only: when it does not compile here, the hit is shown without
// highlight (the engine's own match is the reference).

import type { SearchHit } from '$lib/ipc/engine';

/** Prefix that asks for a regular expression, as in the TUI (`/search re:<regex>`). */
export const REGEX_PREFIX = 're:';

export interface SearchRequest {
	text: string;
	regex: boolean;
}

/** What the user typed, as the engine takes it. The prefix wins over the switch. */
export function readSearchInput(raw: string, regexSwitch: boolean): SearchRequest {
	const lead = raw.trim();
	if (lead.startsWith(REGEX_PREFIX)) return { text: lead.slice(REGEX_PREFIX.length).trim(), regex: true };
	return { text: lead, regex: regexSwitch };
}

export interface FileHits {
	path: string;
	hits: SearchHit[];
}

/** Hits grouped by file, in the order the engine gave them. */
export function groupByFile(hits: SearchHit[]): FileHits[] {
	const groups = new Map<string, SearchHit[]>();
	for (const hit of hits) {
		const list = groups.get(hit.path);
		if (list) list.push(hit);
		else groups.set(hit.path, [hit]);
	}
	return [...groups].map(([path, list]) => ({ path, hits: list }));
}

export interface Segment {
	text: string;
	match: boolean;
}

function escapeRegExp(s: string): string {
	return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/** The line split around the query, for highlighting. Case-sensitive, as the engine. */
export function highlight(line: string, query: string, regex: boolean): Segment[] {
	const whole: Segment[] = [{ text: line, match: false }];
	if (query === '') return whole;
	let pattern: RegExp;
	try {
		pattern = new RegExp(regex ? query : escapeRegExp(query), 'g');
	} catch {
		return whole;
	}
	const out: Segment[] = [];
	let last = 0;
	for (const m of line.matchAll(pattern)) {
		if (m[0] === '') continue;
		const start = m.index ?? 0;
		if (start > last) out.push({ text: line.slice(last, start), match: false });
		out.push({ text: m[0], match: true });
		last = start + m[0].length;
	}
	if (last < line.length) out.push({ text: line.slice(last), match: false });
	return out.length > 0 ? out : whole;
}

export type StatusTone = 'success' | 'warning' | 'muted' | 'destructive';

export interface SearchStatusView {
	label: string;
	tone: StatusTone;
}

const STATUS: Record<string, SearchStatusView> = {
	complete: { label: 'Recherche terminée', tone: 'success' },
	partial: { label: 'Résultats partiels : l’absence ne prouve rien', tone: 'warning' },
	cancelled: { label: 'Recherche annulée', tone: 'muted' },
	error: { label: 'Erreur de recherche', tone: 'destructive' }
};

/** Label and tone of the engine's `search_results.status`. */
export function searchStatus(status: string): SearchStatusView {
	return STATUS[status] ?? { label: `Statut inconnu : ${status}`, tone: 'muted' };
}
