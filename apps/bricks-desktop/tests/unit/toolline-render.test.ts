// Server render of the one-line tool call (ToolLine.svelte): what the line shows, and what it
// must not show (arguments, command text, output)..2, Haiku 4 (validation).
// Node environment: the component is compiled for the server and rendered to a string.
import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import ToolLine from '../../src/lib/features/conversation/timeline/ToolLine.svelte';
import type { ToolLineState } from '../../src/lib/features/conversation/timeline/toolLine';

/** Narrow no-break spaces from fr-FR grouping are in the product; the tests compare plain text. */
const plain = (s: string) => s.replace(/[\u00a0\u202f]/g, ' ');

function line(props: { name: string; state: ToolLineState; durationMs?: number }): string {
	const { body } = render(ToolLine, { props });
	return plain(body.replace(/<!--[^]*?-->/g, '').replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ').trim());
}

describe('ToolLine: what the line shows', () => {
	it('shows the real tool name, its French state and the duration in ms once finished', () => {
		expect(line({ name: 'Bash', state: 'done', durationMs: 6_100 })).toBe('Bash Terminé 6 100 ms');
		expect(line({ name: 'Read', state: 'failed', durationMs: 15 })).toBe('Read Échec 15 ms');
	});

	it('shows the state without a duration when the engine gave none', () => {
		expect(line({ name: 'Edit', state: 'done' })).toBe('Edit Terminé');
		expect(line({ name: 'Edit', state: 'failed' })).toBe('Edit Échec');
	});

	it('shows no duration while the call runs, even if one is known', () => {
		expect(line({ name: 'Bash', state: 'running', durationMs: 900 })).toBe('Bash En cours');
	});

	it('shows interruptions as Interrompu', () => {
		expect(line({ name: 'Bash', state: 'interrupted' })).toBe('Bash Interrompu');
	});
});

describe('ToolLine: what the line never shows', () => {
	it('carries no command, path or argument text: the component has no such input', () => {
		// The props are the name, the state and the duration only. Anything else is ignored.
		const html = render(ToolLine, {
			props: {
				name: 'Bash',
				state: 'done',
				durationMs: 42,
				// Not props of the line: must not appear in its output.
				input: { command: 'cat ~/secret-file' },
				output: 'SORTIE-SECRETE-1234'
			} as unknown as { name: string; state: ToolLineState; durationMs: number }
		}).body;
		expect(html).not.toContain('cat ~/secret-file');
		expect(html).not.toContain('SORTIE-SECRETE-1234');
	});

	it('is a single light row: no card border, no button', () => {
		const html = render(ToolLine, { props: { name: 'Read', state: 'done' } }).body;
		expect(html).not.toMatch(/<button/);
		expect(html).not.toMatch(/\brounded-lg\b/);
		expect(html).not.toMatch(/\bborder\b/);
	});

	it('hides the state icon from assistive technology and keeps the text as the name', () => {
		const html = render(ToolLine, { props: { name: 'Grep', state: 'running' } }).body;
		// Every icon is decorative (aria-hidden), the words carry the meaning.
		const icons = html.match(/<svg[^>]*>/g) ?? [];
		expect(icons.length).toBeGreaterThan(0);
		for (const icon of icons) expect(icon).toContain('aria-hidden="true"');
	});
});
