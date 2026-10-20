// Server render of the one-line tool call (ToolLine.svelte): what the line shows, and what it
// must not show (arguments, command text, output)..2, Haiku 4 (validation).
// Node environment: the component is compiled for the server and rendered to a string.
import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import ToolRow from '../../src/lib/features/conversation/timeline/ToolRow.svelte';
import TimelineEntry from '../../src/lib/features/conversation/timeline/TimelineEntry.svelte';
import { Conversation, type ToolItem, type EditItem } from '../../src/lib/state/conversation.svelte';
import { EnvelopeFeed, PROJECT_ID, openedSession } from './fixtures';
import ToolLine from '../../src/lib/features/conversation/timeline/ToolLine.svelte';
import type { ToolLineState } from '../../src/lib/features/conversation/timeline/toolLine';

/** Narrow no-break spaces from fr-FR grouping are in the product; the tests compare plain text. */
const plain = (s: string) => s.replace(/[\u00a0\u202f]/g, ' ');

function line(props: { name: string; state: ToolLineState; durationMs?: number }): string {
	const { body } = render(ToolLine, { props });
	return plain(body.replace(/<!--[^]*?-->/g, '').replace(/<span class="sr-only">[^]*?<\/span>/g, '').replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ').trim());
}

describe('ToolLine: what the line shows', () => {
	it('shows the real tool name, its French state and the compact duration once finished', () => {
		expect(line({ name: 'Bash', state: 'done', durationMs: 6_100 })).toBe('Bash 6s');
		expect(line({ name: 'Read', state: 'failed', durationMs: 15 })).toBe('Read 15ms');
	});

	it('shows the state without a duration when the engine gave none', () => {
		expect(line({ name: 'Edit', state: 'done' })).toBe('Edit');
		expect(line({ name: 'Edit', state: 'failed' })).toBe('Edit');
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


describe('inline edit result', () => {
	function appliedEdit() {
		const conversation = new Conversation(openedSession(), PROJECT_ID);
		const feed = new EnvelopeFeed({ runId: 'run-edit' });
		conversation.apply(feed.next({ type: 'run_started', prompt: 'Fix', attachments: [], model: 'm' }));
		conversation.apply(feed.next({ type: 'tool_started', tool_call_id: 'edit-1', name: 'Edit', input: { file_path: '/work/properties.rs' } }));
		conversation.apply(feed.next({ type: 'edit_applied', tool_call_id: 'edit-1', tool: 'Edit', files: [
			{ path: '/work/properties.rs', kind: 'modify', added: 1, removed: 2 }
		] }));
		conversation.apply(feed.next({ type: 'tool_finished', tool_call_id: 'edit-1', name: 'Edit', is_error: false, duration_ms: 2000, output: 'ok', executed: true, status: 'success' }));
		const tool = conversation.items.find((i): i is ToolItem => i.kind === 'tool')!;
		const edit = conversation.items.find((i): i is EditItem => i.kind === 'edit')!;
		return { conversation, tool, edit };
	}

	it('puts the file and actual line counts after Edit without a separate card', () => {
		const { conversation, tool, edit } = appliedEdit();
		const html = render(ToolRow, { props: { item: tool, conversation } }).body;
		const visible = html.replace(/<span class="sr-only">[^]*?<\/span>/g, '').replace(/<!--[^]*?-->/g, '').replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ').trim();
		expect(visible).toBe('Edit properties.rs +1 −2 2s');
		expect(html).toContain('title="/work/properties.rs · Modifié"');
		expect(html).not.toMatch(/\bborder\b/);
		expect(render(TimelineEntry, { props: { item: edit, conversation } }).body.replace(/<!--[^]*?-->/g, '').trim()).toBe('');
	});

	it('does not attach another run’s edit to a reused tool id', () => {
		const { conversation, tool, edit } = appliedEdit();
		edit.runId = 'another-run';
		const html = render(ToolRow, { props: { item: tool, conversation } }).body;
		expect(html).not.toContain('properties.rs');
		const standalone = render(TimelineEntry, { props: { item: edit, conversation } }).body;
		expect(standalone).toContain('/work/properties.rs');
		expect(standalone).not.toMatch(/\bborder\b/);
	});
});
