// Skills chosen for the next message: which listed skills are offered, how the
// selection toggles, what stays selected after a run, and the project trust state.
import { describe, expect, it } from 'vitest';
import type { SkillListing, SkillsListing } from '../../src/lib/ipc/engine';
import {
	hasProjectSkillsAwaitingApproval,
	selectionAfterRun,
	toggleSkill,
	usableSkills
} from '../../src/lib/features/composer/skills';

const entry = (name: string, state: string, description?: string): SkillListing => ({
	name,
	source: 'user',
	state,
	description
});

describe('usableSkills: only what the engine lets a message use', () => {
	it('offers available and active skills, marking the active ones', () => {
		expect(
			usableSkills([entry('pdf', 'available', 'Lit les PDF'), entry('tests', 'active'), entry('x', 'disabled')])
		).toEqual([
			{ name: 'pdf', description: 'Lit les PDF', active: false },
			{ name: 'tests', description: null, active: true }
		]);
	});

	it('does not offer invalid, incompatible, refused, pending, shadowed or disabled skills', () => {
		const states = ['invalid', 'incompatible', 'refused', 'pending_approval', 'shadowed', 'disabled'];
		expect(usableSkills(states.map((s, i) => entry(`s${i}`, s)))).toEqual([]);
	});

	it('keeps one choice per name, the first the engine lists', () => {
		const out = usableSkills([entry('dup', 'available', 'premier'), entry('dup', 'available', 'second')]);
		expect(out).toEqual([{ name: 'dup', description: 'premier', active: false }]);
	});
});

describe('toggleSkill and selectionAfterRun', () => {
	it('adds a missing name and removes a chosen one, without touching the others', () => {
		expect(toggleSkill([], 'pdf')).toEqual(['pdf']);
		expect(toggleSkill(['pdf', 'tests'], 'pdf')).toEqual(['tests']);
		expect(toggleSkill(['pdf'], 'tests')).toEqual(['pdf', 'tests']);
	});

	it('clears the skills sent with a run and keeps the ones chosen since', () => {
		expect(selectionAfterRun(['pdf', 'tests'], ['pdf'])).toEqual(['tests']);
		expect(selectionAfterRun(['pdf'], ['pdf'])).toEqual([]);
		expect(selectionAfterRun(['pdf'], [])).toEqual(['pdf']);
	});
});

describe('hasProjectSkillsAwaitingApproval', () => {
	const listing = (entries: SkillListing[]): SkillsListing => ({
		generation: 1,
		project_decision: 'pending',
		entries,
		total: entries.length,
		page: 0,
		catalog_omitted: 0
	});

	it('is true only when a project skill waits for the trust decision', () => {
		expect(hasProjectSkillsAwaitingApproval(listing([entry('p', 'pending_approval')]))).toBe(true);
		expect(hasProjectSkillsAwaitingApproval(listing([entry('u', 'available')]))).toBe(false);
	});
});
