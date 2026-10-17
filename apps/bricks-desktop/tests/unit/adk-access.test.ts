// Unit tests for the right panel's « Plus » menu data (adkAccess.ts): every
// existing inspector view is offered once, under a label of the dialog title.
import { describe, expect, it } from 'vitest';
import { ADK_ACCESS_GROUPS, adkAccessViews } from '../../src/lib/features/context/adkAccess';

describe('adkAccess: right panel menu', () => {
	it('offers the existing ADK inspectors, each exactly once', () => {
		const views = adkAccessViews();
		expect(new Set(views).size).toBe(views.length);
		expect([...views].sort()).toEqual(
			[
				'agent_instances',
				'agents',
				'config',
				'context',
				'cost',
				'jobs',
				'mcp',
				'memory',
				'search',
				'session',
				'tools'
			].sort()
		);
	});

	it('keeps group headings and item labels non-empty and free of duplicated labels', () => {
		const labels: string[] = [];
		for (const group of ADK_ACCESS_GROUPS) {
			expect(group.label.trim()).not.toBe('');
			expect(group.items.length).toBeGreaterThan(0);
			for (const item of group.items) {
				expect(item.label.trim()).not.toBe('');
				labels.push(item.label);
			}
		}
		expect(new Set(labels).size).toBe(labels.length);
	});

	it('uses the dialog titles of InspectorHost for the entries', () => {
		const label = (view: string) =>
			ADK_ACCESS_GROUPS.flatMap((g) => g.items).find((i) => i.view === view)?.label;
		expect(label('session')).toBe('Session');
		expect(label('agent_instances')).toBe('Sous-agents');
		expect(label('agents')).toBe('Profils d’agents');
		expect(label('jobs')).toBe('Tâches de fond');
		expect(label('search')).toBe('Recherche dans le code');
	});
});
