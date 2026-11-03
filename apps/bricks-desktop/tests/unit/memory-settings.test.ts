// Static checks of the memory settings (sources read, not rendered): the section is in the
// settings, the toggle only changes the switch, every deletion asks first and keeps the
// conversations, the retired memories are listed and removable, and the counts read in French.
// Same approach as context-card-structure.test.ts.
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { t } from '../../src/lib/i18n/i18n.svelte';
import { bound, call, tkey } from './french-keys';

const src = (path: string) => readFileSync(new URL(`../../src/${path}`, import.meta.url), 'utf8');

const SETTINGS = 'lib/features/settings/MemorySettings.svelte';
const DIALOG = 'lib/features/settings/SettingsDialog.svelte';

const KEPT = 'L’historique des conversations est conservé.';

describe('the settings section', () => {
	const dialog = src(DIALOG);

	it('is one section of the settings, named Mémoire, showing MemorySettings', () => {
		expect(dialog).toContain(`{ id: 'memory', get label() { return ${call('Mémoire')} }, icon: Brain }`);
		expect(dialog).toContain('<Tabs.Content value="memory"');
		expect(dialog).toMatch(/MemorySettings/);
	});

	it('loads the overview for the open project, or for no project', () => {
		const settings = src(SETTINGS);
		expect(settings).toContain('ipc.memoryOverview(project?.id ?? null)');
		expect(settings).toContain('const project = $derived(app.currentProject && !app.currentProject.chats ? app.currentProject : null);');
	});
});

describe('the switch', () => {
	const settings = src(SETTINGS);

	it('shows the saved state on one switch, labelled Mémoire', () => {
		expect(settings).toContain(`{${call('Mémoire')}}\n\t\t\t<Switch bind:checked={() => app.prefs.memory_enabled, (on) => toggle(on)} disabled={toggling} />`);
		expect(settings).not.toContain('<Badge');
		expect(settings).not.toContain('Désactiver');
		expect(settings).not.toContain('Activer la mémoire');
	});

	it('only saves the preference: turning it off deletes nothing', () => {
		const toggle = settings.slice(settings.indexOf('async function toggle('), settings.indexOf('function total('));
		expect(toggle).toContain('app.setMemoryEnabled(on)');
		expect(toggle).not.toContain('memoryDelete');
	});

	it('shows a refused save as an alert with its reason', () => {
		expect(settings).toContain(`${tkey('Le réglage n’a pas été enregistré : {saveError}')}, { saveError: toggleError })`);
		expect(settings).toMatch(/role="alert"[^>]*>\s*\{t\('settings\.theSettingWasntSaved'/);
	});
});

describe('the counts', () => {
	it('names each count in French with its singular and plural', () => {
		expect(t('settings.noteCount', { count: 1 })).toBe('1 note');
		expect(t('settings.noteCount', { count: 0 })).toBe('0 note');
		expect(t('settings.exchangeCount', { count: 2 })).toBe('2 échanges');
		expect(t('settings.factCount', { count: 1 })).toBe('1 fait');
	});

	it('lists notes, exchanges and facts in that order', () => {
		const settings = src(SETTINGS);
		const body = settings.slice(settings.indexOf('function total('), settings.indexOf('function view('));
		const notes = body.indexOf("t('settings.noteCount', { count: c.notes })");
		const exchanges = body.indexOf("t('settings.exchangeCount', { count: c.exchanges })");
		const facts = body.indexOf("t('settings.factCount', { count: c.facts })");
		expect(notes).toBeGreaterThan(-1);
		expect(exchanges).toBeGreaterThan(notes);
		expect(facts).toBeGreaterThan(exchanges);
	});
});

describe('the global and project memories', () => {
	const settings = src(SETTINGS);

	it('shows each memory under its title, with a dash until its counts are read', () => {
		expect(settings).toContain(bound('title', 'Mémoire globale'));
		expect(settings).toContain(bound('title', 'Mémoire par projet'));
		expect(settings).toContain("{overview ? total(overview.global) : '—'}");
		expect(settings).toContain("{overview?.project ? total(overview.project) : '—'}");
	});

	it('names the missing project, and disables the project actions without one', () => {
		expect(settings).toContain(call('Aucun projet ouvert'));
		expect(settings).toContain("disabled={!project} onclick={() => view('project')}");
		expect(settings).toContain('disabled={!project || !overview?.project}');
	});

	it('opens the memory view for the scope chosen', () => {
		expect(settings).toContain("ui.open('memory', { scope })");
		expect(settings).toContain("onclick={() => view('global')}");
	});

	it('says the bricks.toml [memory] section is not used, when it is present', () => {
		expect(settings).toContain('{#if overview?.toml_section_ignored}');
		expect(settings).toContain(call('La section [memory] de bricks.toml n’est pas utilisée par l’app.'));
	});
});

describe('deletions ask first', () => {
	const settings = src(SETTINGS);

	it('gives each deletion its title and says the conversations are kept', () => {
		expect(settings).toContain(`title: ${call('Supprimer la mémoire globale')}`);
		expect(settings).toContain(`title: ${call('Supprimer la mémoire de ce projet')}`);
		expect(settings).toContain(`title: ${call('Supprimer la mémoire de ce projet retiré')}`);
		// The sentence that the conversations are kept lives in the three texts of the catalogue.
		for (const text of [
			'{c}. Tous les projets perdent l’accès à cette mémoire partagée. L’historique des conversations est conservé.',
			'{name} : {c}. L’historique des conversations est conservé.',
			'{id}. L’historique des conversations est conservé.'
		]) {
			expect(settings).toContain(tkey(text));
		}
		expect(KEPT).toBe('L’historique des conversations est conservé.');
	});

	it('says the global memory is shared by every project', () => {
		expect(settings).toContain(tkey('{c}. Tous les projets perdent l’accès à cette mémoire partagée. L’historique des conversations est conservé.'));
	});

	it('runs the matching deletion, and never runs one before confirmation', () => {
		expect(settings).toContain("run: () => ipc.memoryDelete('global', null)");
		expect(settings).toContain("run: () => ipc.memoryDelete('project', id)");
		expect(settings).toContain('run: () => ipc.memoryDeleteRetired(id)');
		// The asks only set the pending deletion: the call happens in confirmed(), from the dialog.
		const asks = settings.slice(settings.indexOf('function askGlobal('), settings.indexOf('</script>'));
		expect(asks).not.toMatch(/await ipc\.memoryDelete/);
		expect(settings).toContain('onConfirm={() => confirmed(p.run)}');
	});

	it('reloads after a deletion, and reports a refused one without closing as success', () => {
		const confirmedFn = settings.slice(settings.indexOf('async function confirmed('), settings.indexOf('function askGlobal('));
		expect(confirmedFn).toContain('await run();');
		expect(confirmedFn).toContain('await load();');
		expect(confirmedFn).toContain('return { ok: false, message: ipc.errorMessage(e), conflict: false };');
	});

	it('closes the pending deletion on close', () => {
		expect(settings).toContain('onClose={() => (pending = null)}');
	});
});

describe('the retired memories', () => {
	const settings = src(SETTINGS);

	it('lists them only when there are some', () => {
		expect(settings).toContain('{#if overview && overview.retired.length > 0}');
		expect(settings).toContain(bound('title', 'Mémoires de projets retirés'));
	});

	it('names a retired project without a name, and keys each row by its id', () => {
		expect(settings).toContain(`{r.name ?? ${call('Projet sans nom')}}`);
		expect(settings).toContain('{#each overview.retired as r (r.project_id)}');
	});

	it('does not show the former store note any more, and reads no former store', () => {
		expect(settings).not.toContain('L’ancienne mémoire (memory.grafeo) est conservée telle quelle, sans migration.');
		expect(settings).not.toContain('memory.grafeo');
		expect(settings).not.toContain('legacy_store');
	});
});

describe('read errors', () => {
	it('shows a failed read as an alert, and keeps the empty dashes', () => {
		const settings = src(SETTINGS);
		expect(settings).toContain(`${tkey('Lecture impossible : {loadError}')}, { loadError })`);
		expect(settings).toMatch(/role="alert"[^>]*>\s*\{t\('settings\.couldNotRead'/);
	});
});
