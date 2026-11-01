// Static checks (sources read, not rendered) of the terminal panel: it offers « Masquer le terminal »
// after « Nouveau terminal », and shows that button only when its parent hands it an onHide callback.
// The sub-agent surfaces (the branch that replaces the inspector) are in agent-surfaces-structure.test.ts.
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const src = (path: string) => readFileSync(new URL(`../../src/${path}`, import.meta.url), 'utf8');

const TERMINAL = 'lib/features/terminal/TerminalPanel.svelte';

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
