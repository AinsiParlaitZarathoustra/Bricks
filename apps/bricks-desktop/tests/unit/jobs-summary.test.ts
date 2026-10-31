// The jobs summary of the « Tâches de fond » section (labels.ts): one line, the states counted in
// French, singular and plural, in a fixed order, and « Aucune tâche » when there is none. Pure
// function over the states of the jobs.
import { describe, expect, it } from 'vitest';
import { jobsSummary } from '../../src/lib/features/catalog/labels';

const job = (state: string) => ({ state });

describe('jobsSummary: no job', () => {
	it('says « Aucune tâche » for an empty list', () => {
		expect(jobsSummary([])).toBe('Aucune tâche');
	});
});

describe('jobsSummary: singular and plural', () => {
	it('counts one job of each state in the singular', () => {
		expect(jobsSummary([job('running')])).toBe('1 en cours');
		expect(jobsSummary([job('completed')])).toBe('1 terminée');
		expect(jobsSummary([job('failed')])).toBe('1 en échec');
		expect(jobsSummary([job('stopped')])).toBe('1 arrêtée');
	});

	it('counts several jobs of each state in the plural', () => {
		expect(jobsSummary([job('running'), job('running')])).toBe('2 en cours');
		expect(jobsSummary([job('completed'), job('completed')])).toBe('2 terminées');
		expect(jobsSummary([job('failed'), job('failed')])).toBe('2 en échec');
		expect(jobsSummary([job('stopped'), job('stopped')])).toBe('2 arrêtées');
	});
});

describe('jobsSummary: several states', () => {
	it('lists the states in a fixed order, whatever the order of the jobs, joined by a middle dot', () => {
		expect(jobsSummary([job('completed'), job('running'), job('completed')])).toBe('1 en cours · 2 terminées');
		expect(jobsSummary([job('stopped'), job('failed'), job('running')])).toBe('1 en cours · 1 en échec · 1 arrêtée');
		expect(jobsSummary([job('stopped'), job('completed')])).toBe('1 terminée · 1 arrêtée');
	});

	it('leaves out the states that no job has', () => {
		expect(jobsSummary([job('completed')])).not.toContain('en cours');
	});

	it('names a state it does not know by its own name, after the known ones', () => {
		expect(jobsSummary([job('weird'), job('running')])).toBe('1 en cours · 1 weird');
	});
});
