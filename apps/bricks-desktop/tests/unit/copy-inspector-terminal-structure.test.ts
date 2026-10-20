// Static checks (sources read, not rendered) of two surfaces of Sprint 14.6: the agent inspector
// shows no identifier or parent label and renders its texts through MarkdownView, and the terminal
// panel offers « Masquer le terminal » after « Nouveau terminal ». The copy of an answer is tested
// on its logic (tests/unit/responses.test.ts).
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const src = (path: string) => readFileSync(new URL(`../../src/${path}`, import.meta.url), 'utf8');

const INSPECTOR = 'lib/features/agents/AgentInspector.svelte';
const TERMINAL = 'lib/features/terminal/TerminalPanel.svelte';

describe('agent inspector', () => {
	const text = src(INSPECTOR);

	it('shows no « Identifiant » and no « Parent » label', () => {
		expect(text).not.toContain('Identifiant');
		expect(text).not.toContain('Parent');
	});

	it('renders its task and its result summary through MarkdownView, not raw text', () => {
		expect(text).toContain("import MarkdownView from '$lib/features/conversation/markdown/MarkdownView.svelte';");
		expect(text).toContain('<MarkdownView source={spawn.task}');
		expect(text).toContain('<MarkdownView source={result.summary}');
		expect(text).not.toContain('{@html');
	});
});

describe('terminal panel', () => {
	const text = src(TERMINAL);

	it('offers « Masquer le terminal » after « Nouveau terminal » in its markup', () => {
		const create = text.indexOf('aria-label="Nouveau terminal"');
		const hide = text.indexOf('aria-label="Masquer le terminal"');
		expect(create).toBeGreaterThan(0);
		expect(hide).toBeGreaterThan(create);
		expect(text).toContain('title="Masquer le terminal"');
	});

	it('shows the hide button only when its parent hands it an onHide callback', () => {
		const hide = text.indexOf('aria-label="Masquer le terminal"');
		expect(text.slice(0, hide)).toContain('{#if onHide}');
		expect(text).toContain('onclick={onHide}');
	});
});
