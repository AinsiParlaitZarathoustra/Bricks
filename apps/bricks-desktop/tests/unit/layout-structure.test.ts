// Static checks (sources read, not rendered) of the top bar and the right panel: the « Plus » menu
// sits after the panel toggles, the context panel keeps no « Plus » and no « Sous-agents » card,
// the environment rows are icons, the run-end cost line keeps its wording, and the window size is
// the one the app asks for. Same approach as removed-texts.test.ts.
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const src = (path: string) => readFileSync(new URL(`../../src/${path}`, import.meta.url), 'utf8');

const TOPBAR = 'lib/layout/TopBar.svelte';
const APP = 'App.svelte';
const PLUS = 'lib/features/context/PlusMenu.svelte';
const CONTEXT_PANEL = 'lib/features/context/ContextPanel.svelte';
const ENVIRONMENT = 'lib/features/context/ProjectEnvironment.svelte';
const RUN_END = 'lib/features/conversation/timeline/RunEndCard.svelte';

describe('top bar: the « Plus » menu comes after the panel toggles', () => {
	it('renders the trailing slot after the « Affichage » group', () => {
		const topbar = src(TOPBAR);
		const group = topbar.indexOf('aria-label="Affichage"');
		const lastToggle = topbar.indexOf('{#if onToggleReader}');
		const trailing = topbar.indexOf('{@render trailing()}');
		expect(group).toBeGreaterThan(-1);
		expect(lastToggle).toBeGreaterThan(group);
		expect(trailing).toBeGreaterThan(lastToggle);
	});

	it('App passes PlusMenu as the trailing snippet of the top bar', () => {
		const app = src(APP);
		expect(app).toMatch(/\{#snippet trailing\(\)\}\s*<PlusMenu \/>\s*\{\/snippet\}/);
		expect(app).toContain("import PlusMenu from '$lib/features/context/PlusMenu.svelte'");
	});
});

describe('the « Plus » menu and the context panel', () => {
	it('PlusMenu is the labelled trigger of the ADK access groups, disabled without a conversation', () => {
		const plus = src(PLUS);
		expect(plus).toContain('ADK_ACCESS_GROUPS');
		expect(plus).toMatch(/<EllipsisIcon[^>]*\/>\s*Plus/);
		expect(plus).toContain('disabled={!enabled}');
	});

	it('the context panel has no « Plus » menu and no « Sous-agents » card', () => {
		const panel = src(CONTEXT_PANEL);
		expect(panel).not.toContain('PlusMenu');
		expect(panel).not.toMatch(/>\s*Plus\s*</);
		expect(panel).not.toContain('Sous-agents');
		expect(panel).not.toContain("'agent_instances'");
	});
});

describe('project environment: icons instead of the Projet, Dossier and Git labels', () => {
	const env = src(ENVIRONMENT);

	it('imports the three icons', () => {
		expect(env).toContain("import FolderKanbanIcon from '@lucide/svelte/icons/folder-kanban';");
		expect(env).toContain("import FolderIcon from '@lucide/svelte/icons/folder';");
		expect(env).toContain("import GitBranchIcon from '@lucide/svelte/icons/git-branch';");
	});

	it('names each icon for screen readers with sr-only text', () => {
		for (const name of ['Projet', 'Dossier', 'Git']) {
			expect(env, name).toContain(`<span class="sr-only">${name}</span>`);
		}
	});

	it('has exactly three rows, and no model or reasoning row', () => {
		expect(env.match(/<dt\b/g)?.length).toBe(3);
		expect(env).not.toMatch(/sr-only">(Modèle|Raisonnement)</);
		expect(env).not.toMatch(/>(Modèle|Raisonnement)<\/dt>/);
	});
});

describe('run-end card: no cost line', () => {
	it('the summary of a response shows no cost (it stays in the Consommation view)', () => {
		const card = readFileSync(new URL('../../src/lib/features/conversation/timeline/RunEndCard.svelte', import.meta.url), 'utf8');
		expect(card).not.toContain('Coût de la réponse');
		expect(card).not.toContain('formatRunCost');
	});
});

describe('window size', () => {
	it('opens at 1440 by 900 and stops at 800 by 560', () => {
		const conf = JSON.parse(readFileSync(new URL('../../src-tauri/tauri.conf.json', import.meta.url), 'utf8'));
		const [window] = conf.app.windows;
		expect(window).toMatchObject({ width: 1440, height: 900, minWidth: 800, minHeight: 560 });
	});
});
