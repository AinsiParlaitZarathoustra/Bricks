// Input history: which keys recall entries, and that the draft is kept aside.
import { describe, expect, it } from 'vitest';
import {
	HistoryCursor,
	shouldRecallNewer,
	shouldRecallOlder,
	type CaretState
} from '../../src/lib/features/commands/history';

function caret(text: string, start: number, end = start, browsing = false): CaretState {
	return { text, selectionStart: start, selectionEnd: end, browsing };
}

describe('shouldRecallOlder', () => {
	it('recalls in an empty editor', () => {
		expect(shouldRecallOlder(caret('', 0))).toBe(true);
	});

	it('recalls with the caret at the very start, not in the middle', () => {
		expect(shouldRecallOlder(caret('bonjour', 0))).toBe(true);
		expect(shouldRecallOlder(caret('bonjour', 3))).toBe(false);
	});

	it('keeps going while browsing as long as the caret is on the first line', () => {
		expect(shouldRecallOlder(caret('une ligne', 9, 9, true))).toBe(true);
		expect(shouldRecallOlder(caret('ligne 1\nligne 2', 12, 12, true))).toBe(false);
	});
});

describe('shouldRecallNewer', () => {
	it('never moves forward when no entry is shown', () => {
		expect(shouldRecallNewer(caret('', 0))).toBe(false);
	});

	it('moves forward while browsing with the caret on the last line', () => {
		expect(shouldRecallNewer(caret('une ligne', 9, 9, true))).toBe(true);
		expect(shouldRecallNewer(caret('ligne 1\nligne 2', 0, 0, true))).toBe(false);
	});
});

describe('HistoryCursor', () => {
	const entries = ['a', 'b', 'c'];
	const owner = { name: 'draft owner' };

	it('goes back through the entries, then stops at the oldest', () => {
		const h = new HistoryCursor<typeof owner>();
		expect(h.older(entries, 'brouillon', owner)).toBe('c');
		expect(h.older(entries, 'c', owner)).toBe('b');
		expect(h.older(entries, 'b', owner)).toBe('a');
		expect(h.older(entries, 'a', owner)).toBeNull();
		expect(h.browsing).toBe(true);
	});

	it('goes forward again and gives back the saved draft past the newest entry', () => {
		const h = new HistoryCursor<typeof owner>();
		h.older(entries, 'brouillon en cours', owner); // shows 'c'
		h.older(entries, 'c', owner); // shows 'b'
		expect(h.newer(entries)).toBe('c');
		expect(h.newer(entries)).toBe('brouillon en cours');
		expect(h.browsing).toBe(false);
		expect(h.newer(entries)).toBeNull();
	});

	it('keeps the draft from before the first recall, whatever happens meanwhile', () => {
		const h = new HistoryCursor<typeof owner>();
		h.older(entries, 'mon texte', owner);
		h.older(entries, 'c', owner);
		const saved = h.end();
		expect(saved).toEqual({ owner, text: 'mon texte' });
		expect(h.browsing).toBe(false);
		expect(h.end()).toBeNull();
	});

	it('recalls nothing when the conversation has no earlier prompt', () => {
		const h = new HistoryCursor<typeof owner>();
		expect(h.older([], 'brouillon', owner)).toBeNull();
		expect(h.browsing).toBe(false);
	});
});
