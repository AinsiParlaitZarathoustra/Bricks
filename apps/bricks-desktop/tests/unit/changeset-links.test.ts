// Unit tests for the changes panel's links to ChangeSets (catalog/changeset-links.ts).
import { describe, expect, it } from 'vitest';
import { changesetIdsOf, changesetLinkLabel } from '../../src/lib/features/catalog/changeset-links';

describe('changesetIdsOf', () => {
	it('returns the distinct changesets in order of first appearance', () => {
		const ids = changesetIdsOf([
			{ changesetId: 'cs-a' },
			{ changesetId: null },
			{ changesetId: 'cs-b' },
			{ changesetId: 'cs-a' }
		]);
		expect(ids).toEqual(['cs-a', 'cs-b']);
	});

	it('is empty when no operation belongs to a changeset', () => {
		expect(changesetIdsOf([{ changesetId: null }, { changesetId: null }])).toEqual([]);
		expect(changesetIdsOf([])).toEqual([]);
	});
});

describe('changesetLinkLabel', () => {
	it('uses the plain label when the file has one changeset', () => {
		expect(changesetLinkLabel(['cs-a'], 'cs-a')).toBe('Voir les modifications isolées');
	});

	it('adds the short id of each changeset when there are several', () => {
		const label = changesetLinkLabel(['0123456789abcdef', 'fedcba9876543210'], 'fedcba9876543210');
		expect(label).toBe('Voir les modifications isolées · fedcba98');
	});
});
