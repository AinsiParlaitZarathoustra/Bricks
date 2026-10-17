// The « Historique » tab: an empty journal shows nothing under the tab, a real error
// stays visible with its retry, and an error of « Charger plus » does not hide the records.
import { describe, expect, it } from 'vitest';
import { historyView } from '../../src/lib/features/changes/history-state';

const base = { hasProject: true, loaded: true, error: null, count: 3 };

describe('historyView', () => {
	it('shows the entries of a loaded journal, without any header', () => {
		expect(historyView(base)).toEqual({ view: 'entries', inlineError: null });
	});

	it('shows nothing under the tab when the journal is empty and loaded', () => {
		expect(historyView({ ...base, count: 0 })).toEqual({ view: 'empty', inlineError: null });
	});

	it('shows the loading state until the first load completes', () => {
		expect(historyView({ ...base, loaded: false, count: 0 })).toEqual({ view: 'loading', inlineError: null });
	});

	it('shows the failure with its retry when nothing could be loaded', () => {
		expect(historyView({ ...base, loaded: false, count: 0, error: 'disque illisible' })).toEqual({
			view: 'failed',
			inlineError: null
		});
	});

	it('keeps the records and reports a failed « Charger plus » above them', () => {
		expect(historyView({ ...base, error: 'réseau' })).toEqual({ view: 'entries', inlineError: 'réseau' });
	});

	it('asks for a project first', () => {
		expect(historyView({ ...base, hasProject: false })).toEqual({ view: 'no_project', inlineError: null });
	});
});
