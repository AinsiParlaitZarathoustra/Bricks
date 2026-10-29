// Unit tests for the skills settings: paging of the engine listing, and the French labels and tones
// of the sources and states it reports.
import { describe, expect, it } from 'vitest';
import {
	SKILLS_PER_PAGE,
	clampPage,
	isInstalledSkill,
	pageCount,
	skillCountLabel,
	skillSourceLabel,
	skillStateLabel,
	skillStateTone
} from '../../src/lib/features/settings/skills';

describe('paging', () => {
	it('uses the engine page size of 20', () => {
		expect(SKILLS_PER_PAGE).toBe(20);
	});

	it('counts pages, with one page even when nothing matches', () => {
		expect(pageCount(0)).toBe(1);
		expect(pageCount(1)).toBe(1);
		expect(pageCount(20)).toBe(1);
		expect(pageCount(21)).toBe(2);
		expect(pageCount(41)).toBe(3);
	});

	it('keeps a page inside the listing', () => {
		expect(clampPage(5, 21)).toBe(1);
		expect(clampPage(-1, 21)).toBe(0);
		expect(clampPage(3, 0)).toBe(0);
		expect(clampPage(1, 45)).toBe(1);
	});

	it('names a count of skills in French', () => {
		expect(skillCountLabel(0)).toBe('0 skills');
		expect(skillCountLabel(1)).toBe('1 skill');
		expect(skillCountLabel(7)).toBe('7 skills');
	});
});

describe('states', () => {
	it('labels every state the engine reports', () => {
		const states = [
			'available',
			'active',
			'shadowed',
			'invalid',
			'incompatible',
			'pending_approval',
			'refused',
			'disabled'
		];
		for (const state of states) {
			expect(skillStateLabel(state)).not.toBe(state);
		}
		expect(skillStateLabel('pending_approval')).toBe('Approbation requise');
	});

	it('shows an unknown state as the engine wrote it', () => {
		expect(skillStateLabel('future_state')).toBe('future_state');
	});

	it('marks usable states as ok, states needing attention as warn, the rest as muted', () => {
		expect(skillStateTone('available')).toBe('ok');
		expect(skillStateTone('active')).toBe('ok');
		expect(skillStateTone('refused')).toBe('warn');
		expect(skillStateTone('invalid')).toBe('warn');
		expect(skillStateTone('shadowed')).toBe('muted');
		expect(skillStateTone('disabled')).toBe('muted');
		expect(skillStateTone('unknown')).toBe('muted');
	});
});

describe('sources', () => {
	it('names the catalogue sources', () => {
		expect(skillSourceLabel('builtin')).toBe('Intégrée');
		expect(skillSourceLabel('user')).toBe('Personnelle');
		expect(skillSourceLabel('project')).toBe('Projet');
	});

	it('names the engine sources with their folder', () => {
		expect(skillSourceLabel('user:.agents')).toBe('Personnelle · .agents');
		expect(skillSourceLabel('project:.claude')).toBe('Projet · .claude');
		expect(skillSourceLabel('sdk:tools')).toBe('SDK · tools');
	});

	it('keeps an unknown source as it is', () => {
		expect(skillSourceLabel('elsewhere')).toBe('elsewhere');
	});

	it('names the skills installed in the store, whatever their installation', () => {
		expect(skillSourceLabel('store')).toBe('Installée');
		expect(skillSourceLabel('store:3f9a0c1b2d4e')).toBe('Installée');
		expect(isInstalledSkill('store:3f9a0c1b2d4e')).toBe(true);
		expect(isInstalledSkill('store')).toBe(true);
		expect(isInstalledSkill('builtin')).toBe(false);
		expect(isInstalledSkill('storehouse')).toBe(false);
		expect(isInstalledSkill('user:.agents')).toBe(false);
	});
});
