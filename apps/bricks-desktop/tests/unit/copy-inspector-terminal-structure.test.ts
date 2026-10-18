// Static checks (sources read, not rendered) of three surfaces of Sprint 14.6: the copy button of an
// assistant answer copies its Markdown source only, the agent inspector shows no identifier or
// parent label and renders its texts through MarkdownView, and the terminal panel offers « Masquer
// le terminal » after « Nouveau terminal ».
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const src = (path: string) => readFileSync(new URL(`../../src/${path}`, import.meta.url), 'utf8');

const ASSISTANT = 'lib/features/conversation/timeline/AssistantMessage.svelte';
const INSPECTOR = 'lib/features/agents/AgentInspector.svelte';
const TERMINAL = 'lib/features/terminal/TerminalPanel.svelte';

describe('copy button of an assistant answer', () => {
	const text = src(ASSISTANT);

	it('copies the Markdown source of the item, and nothing else', () => {
		expect(text).toContain('await navigator.clipboard.writeText(item.text)');
		// The copy never takes the rendered text, the throttled stream copy or a DOM node.
		expect(text).not.toMatch(/writeText\((?!item\.text\))/);
		expect(text).not.toMatch(/writeText\([^)]*throttled/);
		expect(text).not.toContain('innerText');
		expect(text).not.toContain('textContent');
	});

	it('is shown only once the answer is complete, with its accessible name', () => {
		const button = text.indexOf('onclick={copyAnswer}');
		expect(button).toBeGreaterThan(0);
		expect(text.slice(0, button)).toContain('{#if !item.streaming}');
		expect(text).toContain('aria-label="Copier"');
	});

	it('reports a failed copy in French, without losing the answer', () => {
		expect(text).toContain('Copie impossible : {copyError}');
		expect(text).toContain("copied = false");
	});
});

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
