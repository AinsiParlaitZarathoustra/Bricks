// Pure helpers for the navigation feature: dates, folder labels and search
// highlighting. No Svelte or IPC imports, so they stay easy to test.

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/** Largest unit first: the first one that fits the distance is used. */
const RELATIVE_UNITS: Array<[Intl.RelativeTimeFormatUnit, number]> = [
	['year', 365 * DAY],
	['month', 30 * DAY],
	['week', 7 * DAY],
	['day', DAY],
	['hour', HOUR],
	['minute', MINUTE]
];

const relativeFormat = new Intl.RelativeTimeFormat('fr', { numeric: 'auto' });

/**
 * French relative date for a millisecond timestamp, e.g. « il y a 5 minutes »,
 * « hier », « maintenant ». Timestamps come from the engine in milliseconds.
 */
export function relativeDate(timestampMs: number, now: number = Date.now()): string {
	const diff = timestampMs - now;
	const distance = Math.abs(diff);
	for (const [unit, size] of RELATIVE_UNITS) {
		if (distance >= size) return relativeFormat.format(Math.trunc(diff / size), unit);
	}
	return relativeFormat.format(0, 'second');
}

/** Title to show for a conversation; an untitled one gets a French placeholder. */
export function displayTitle(title: string): string {
	return title.trim() || 'Sans titre';
}

/** Last `segments` parts of a folder path, with an ellipsis when it is shortened. */
export function shortPath(path: string, segments = 2): string {
	const parts = path.split(/[\\/]/).filter(Boolean);
	if (parts.length <= segments) return path;
	return `…/${parts.slice(-segments).join('/')}`;
}

export interface TextPart {
	text: string;
	/** True for a fragment that matches the query. */
	match: boolean;
}

function escapeRegExp(value: string): string {
	return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/**
 * Splits `text` around case-insensitive occurrences of `query`. The caller renders
 * the parts as text nodes, so no HTML is ever injected.
 */
export function highlightParts(text: string, query: string): TextPart[] {
	const needle = query.trim();
	if (!needle) return [{ text, match: false }];
	const pattern = new RegExp(`(${escapeRegExp(needle)})`, 'gi');
	return text
		.split(pattern)
		.map((part, index) => ({ text: part, match: index % 2 === 1 }))
		.filter((part) => part.text.length > 0);
}
