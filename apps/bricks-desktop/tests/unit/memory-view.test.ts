// Static checks of the memory view (source read, not rendered): the pages of 50, the filters
// and their French labels, the search that waits for typing and drops stale answers, the scope
// rules, the reading that works whether the memory is on or off, and how an entry names its
// project and conversation. Same approach as context-card-structure.test.ts.
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { formatInt } from '../../src/lib/features/inspect/format';

const src = (path: string) => readFileSync(new URL(`../../src/${path}`, import.meta.url), 'utf8');

const VIEW = 'lib/features/inspect/MemoryView.svelte';
const view = src(VIEW);

describe('pages', () => {
	it('reads 50 entries a page, the engine maximum', () => {
		expect(view).toContain('const PAGE = 50;');
		expect(view).toContain('PAGE\n\t\t\t\t);');
	});

	it('moves back and forth by one page, never before the first entry', () => {
		expect(view).toContain('onclick={() => (offset = Math.max(0, offset - PAGE))}');
		expect(view).toContain('onclick={() => (offset += PAGE)}');
		expect(view).toContain('disabled={offset === 0}');
		expect(view).toContain('disabled={offset + PAGE >= page.total}');
	});

	it('says which entries the page shows out of the total', () => {
		expect(view).toContain('{formatInt(offset + 1)}–{formatInt(offset + page.entries.length)} sur {formatInt(page.total)}');
		expect(formatInt(1)).toBe(new Intl.NumberFormat('fr-FR').format(1));
		expect(formatInt(1234.4)).toBe(new Intl.NumberFormat('fr-FR').format(1234));
	});

	it('goes back to the first page when the scope, the filter or the search changes', () => {
		expect(view).toContain('function choose(next: () => void) {\n\t\tnext();\n\t\toffset = 0;\n\t}');
		expect(view).toContain('choose(() => (query = event.currentTarget.value))');
		expect(view).toContain('choose(() => (filter = v as MemoryEntryFilter))');
		expect(view).toContain('choose(() => (scope = v as MemoryScope))');
	});
});

describe('filters', () => {
	it('offers the five filters in order, with their engine values and French labels', () => {
		const expected = [
			["'all'", "'Tout'"],
			["'notes'", "'Notes'"],
			["'exchanges'", "'Échanges'"],
			["'facts'", "'Faits'"],
			["'fact_history'", "'Historique des faits'"]
		];
		const start = view.indexOf('const FILTERS');
		const block = view.slice(start, view.indexOf('];', start));
		let at = 0;
		for (const [value, label] of expected) {
			const found = block.indexOf(`value: ${value}, label: ${label}`, at);
			expect(found, `${value} ${label}`).toBeGreaterThan(-1);
			at = found;
		}
	});

	it('explains that no fact is extracted automatically, on the facts filters only', () => {
		expect(view).toContain("const factsShown = $derived(filter === 'facts' || filter === 'fact_history');");
		expect(view).toContain('{#if factsShown}');
		expect(view).toContain('Aucun fait n’est extrait automatiquement.');
	});
});

describe('search', () => {
	it('waits 250 ms after typing, and reads at once with no search text', () => {
		expect(view).toContain('}, read.query ? 250 : 0);');
	});

	it('drops an answer that a newer request has replaced', () => {
		const count = view.split('if (id !== request) return;').length - 1;
		expect(count).toBe(2);
	});

	it('labels the search field for screen readers', () => {
		expect(view).toContain('aria-label="Rechercher dans la mémoire"');
	});
});

describe('scope', () => {
	it('reads the global memory when asked for it, or when no project is open', () => {
		expect(view).toContain(
			"let scope = $derived<MemoryScope>(args.scope === 'global' || !project ? 'global' : 'project');"
		);
	});

	it('sends the project only for the project scope', () => {
		expect(view).toContain("read.scope === 'project' ? read.projectId : null");
	});

	it('disables the project tab when there is no project, and never reads conversations without one', () => {
		expect(view).toContain('<Tabs.Trigger value="project" disabled={!project}>Projet</Tabs.Trigger>');
		expect(view).toContain('const project = $derived(app.currentProject && !app.currentProject.chats ? app.currentProject : null);');
	});
});

describe('reading is independent of the switch', () => {
	it('never checks whether the memory is on', () => {
		expect(view).not.toContain('memory_enabled');
		expect(view).not.toContain('setMemoryEnabled');
	});
});

describe('entries', () => {
	it('names each kind in French', () => {
		expect(view).toContain("note: 'Note',");
		expect(view).toContain("exchange: 'Échange',");
		expect(view).toContain("fact: 'Fait'");
	});

	it('names the role of an exchange and the status of a fact in French', () => {
		expect(view).toContain("user: 'message',");
		expect(view).toContain("assistant: 'réponse',");
		expect(view).toContain("tool: 'outil'");
		for (const [status, label] of [
			['active', 'actif'],
			['proposed', 'proposé'],
			['superseded', 'remplacé'],
			['contested', 'contesté'],
			['retracted', 'retiré'],
			['unsupported', 'sans source']
		]) {
			expect(view).toContain(`${status}: '${label}'`);
		}
	});

	it('shows the evidence of a fact as quotes, and the time of the entry', () => {
		expect(view).toContain('« {proof.quote} »');
		expect(view).toContain('<time datetime={new Date(entry.at).toISOString()}>{when(entry.at)}</time>');
	});

	it('says where a global entry comes from: its project, then its conversation', () => {
		const start = view.indexOf('function source(');
		const body = view.slice(start, view.indexOf('</script>'));
		expect(body).toContain("if (scope === 'global' && e.project_id) parts.push(`Projet ${e.project_name ?? e.project_id}`);");
		expect(body).toContain("parts.push(conversation(e.session_id) ?? 'Conversation fermée')");
		expect(body.indexOf('Projet')).toBeLessThan(body.indexOf('Conversation fermée'));
	});

	it('names an untitled conversation of the loaded lists', () => {
		expect(view).toContain("return found.title || 'Sans titre';");
	});
});

describe('empty and failed reads', () => {
	it('says there is no memory when the page is empty, and shows a failed read as an alert', () => {
		expect(view).toContain('<p class="text-ui text-muted-foreground">Aucune mémoire</p>');
		expect(view).toContain('<p role="alert" class="text-ui text-pretty text-destructive">{loadError}</p>');
	});

	it('shows placeholders until the first page arrives', () => {
		expect(view).toContain('{:else if page === null}');
		expect(view).toContain('<Skeleton class="h-14 w-full" />');
	});
});
