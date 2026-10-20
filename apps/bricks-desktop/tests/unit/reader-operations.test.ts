// The reader's modes and operations: Markdown gets a preview, edited files get
// their recorded edits (each on its own, in order), and a patch that was not
// recorded or was cut stays unavailable instead of being rebuilt.
import { describe, expect, it } from 'vitest';
import { ReaderStore } from '../../src/lib/state/reader.svelte';
import { PROJECT, SESSION, WORKTREE, diskRead, editItem, settle } from './reader-fixtures';
import { timeline } from './reader-timeline.svelte';

const target = { sessionId: SESSION, projectId: 'p1' };
const DIFF_A = '@@ -1 +1 @@\n-a\n+b\n';
const DIFF_B = '@@ -1 +1 @@\n-x\n+y\n';

/** A reader over `files` whose timeline is `items`, which the test changes. */
function setup(files: Record<string, string[]>) {
	const { read } = diskRead(files);
	const items = timeline();
	const reader = new ReaderStore(read);
	reader.connect(() => items);
	return { reader, items };
}

describe('modes', () => {
	it('Markdown files offer the preview, other files do not', async () => {
		const { reader } = setup({
			[`${PROJECT}/docs/a.md`]: ['# a'],
			[`${PROJECT}/docs/b.markdown`]: ['# b'],
			[`${PROJECT}/docs/c.mdx`]: ['# c'],
			[`${PROJECT}/src/a.rs`]: ['fn a() {}'],
		});
		for (const path of ['docs/a.md', 'docs/b.markdown', 'docs/c.mdx']) {
			const tab = reader.open({ ...target, path });
			expect(tab.isMarkdown, path).toBe(true);
			expect(tab.modes, path).toEqual(['source', 'preview']);
		}
		const rust = reader.open({ ...target, path: 'src/a.rs' });
		expect(rust.isMarkdown).toBe(false);
		expect(rust.modes).toEqual(['source']);
		await settle();
	});

	it('SVG has a read-only preview while JavaScript stays source-only', async () => {
        const { reader } = setup({ [`${PROJECT}/plot.svg`]: ['<svg/>'], [`${PROJECT}/script.js`]: ['alert(1)'] });
        const svg = reader.open({ ...target, path: 'plot.svg' });
        expect(svg.modes).toEqual(['source', 'preview']);
        expect(reader.setMode(svg.id, 'preview')).toBe(true);
        const js = reader.open({ ...target, path: 'script.js' });
        expect(js.modes).toEqual(['source']);
        expect(reader.setMode(js.id, 'preview')).toBe(false);
        await settle();
    });

	it('the source is shown by default, and the preview once chosen', async () => {
		const { reader } = setup({ [`${PROJECT}/docs/a.md`]: ['# a'] });
		const tab = reader.open({ ...target, path: 'docs/a.md' });
		await settle();
		expect(tab.mode).toBe('source');
		expect(reader.setMode(tab.id, 'preview')).toBe(true);
		expect(tab.mode).toBe('preview');
		expect(reader.setMode(tab.id, 'source')).toBe(true);
		expect(tab.mode).toBe('source');
	});

	it('a mode the file does not offer is refused and not shown', async () => {
		const { reader } = setup({ [`${PROJECT}/src/a.rs`]: ['fn a() {}'] });
		const tab = reader.open({ ...target, path: 'src/a.rs' });
		await settle();
		expect(reader.setMode(tab.id, 'preview')).toBe(false);
		expect(reader.setMode(tab.id, 'changes')).toBe(false);
		expect(tab.mode).toBe('source');
	});

	it('changes is offered while the file has recorded edits, and falls back to the source when they go', async () => {
		const { reader, items } = setup({ [`${PROJECT}/src/a.rs`]: ['b'] });
		items.push(editItem('e1', [{ path: 'src/a.rs', absolute: `${PROJECT}/src/a.rs`, patch: DIFF_A, added: 1, removed: 1 }]));
		const tab = reader.open({ ...target, path: 'src/a.rs' });
		await settle();
		expect(tab.modes).toEqual(['source', 'changes']);
		expect(reader.setMode(tab.id, 'changes')).toBe(true);
		expect(tab.mode).toBe('changes');
		items.splice(0);
		expect(tab.operations).toEqual([]);
		expect(tab.mode).toBe('source');
	});
});

describe('operations of a file', () => {
	it('each edit of the file is an operation of its own, in conversation order, never merged', async () => {
		const { reader, items } = setup({ [`${PROJECT}/src/a.rs`]: ['b'] });
		items.push(
			editItem('e1', [{ path: 'src/a.rs', absolute: `${PROJECT}/src/a.rs`, patch: DIFF_A, added: 1, removed: 1 }]),
			editItem('e2', [{ path: 'lib/x.rs', absolute: `${PROJECT}/lib/x.rs`, patch: DIFF_B }]),
			editItem('e3', [{ path: 'src/a.rs', absolute: `${PROJECT}/src/a.rs`, patch: DIFF_B, added: 1, removed: 1 }]),
		);
		const tab = reader.open({ ...target, path: 'src/a.rs' });
		await settle();
		const operations = tab.operations;
		expect(operations.map((op) => op.toolCallId)).toEqual(['tc-e1', 'tc-e3']);
		expect(operations.map((op) => op.patch)).toEqual([DIFF_A, DIFF_B]);
		expect(new Set(operations.map((op) => op.key)).size).toBe(2);
		expect(operations[0]).toMatchObject({ added: 1, removed: 1, truncated: false, agentId: null, kind: 'modify' });
	});

	it('an edit of a file with the same name in another folder is not an operation of this file', async () => {
		const { reader, items } = setup({ [`${PROJECT}/src/a.rs`]: ['b'], [`${PROJECT}/lib/a.rs`]: ['c'] });
		items.push(editItem('e1', [{ path: 'lib/a.rs', absolute: `${PROJECT}/lib/a.rs`, patch: DIFF_B }]));
		const tab = reader.open({ ...target, path: 'src/a.rs' });
		await settle();
		expect(tab.operations).toEqual([]);
	});

	it('an edit with an absolute path matches the file whatever spelling the tab was opened with', async () => {
		const { reader, items } = setup({ [`${PROJECT}/src/a.rs`]: ['b'] });
		items.push(editItem('e1', [{ path: 'a.rs', absolute: `${PROJECT}/src/a.rs`, patch: DIFF_A }]));
		const relative = reader.open({ ...target, path: 'src/a.rs' });
		await settle();
		expect(relative.operations.map((op) => op.toolCallId)).toEqual(['tc-e1']);
		const absolute = reader.open({ ...target, path: `${PROJECT}/src/a.rs`, absolute: `${PROJECT}/src/a.rs` });
		await settle();
		expect(absolute).toBe(relative);
		expect(absolute.operations).toHaveLength(1);
	});

	it('a relative edit matches only a tab of the same agent and the same relative path', async () => {
		const { reader, items } = setup({
			[`${PROJECT}/src/a.rs`]: ['projet'],
			[`${WORKTREE}/src/a.rs`]: ['worktree'],
		});
		items.push(
			editItem('e1', [{ path: 'src/a.rs', patch: DIFF_A }]),
			editItem('e2', [{ path: 'src/a.rs', patch: DIFF_B }], 'a1'),
		);
		const main = reader.open({ ...target, path: 'src/a.rs' });
		const agent = reader.open({ ...target, path: 'src/a.rs', agentId: 'a1' });
		await settle();
		expect(main.operations.map((op) => op.toolCallId)).toEqual(['tc-e1']);
		expect(agent.operations.map((op) => op.toolCallId)).toEqual(['tc-e2']);
		expect(agent.operations[0].agentId).toBe('a1');
	});

	it('a patch that was not recorded is unavailable, not rebuilt from the counts', async () => {
		const { reader, items } = setup({ [`${PROJECT}/src/a.rs`]: ['b'] });
		items.push(editItem('e1', [{ path: 'src/a.rs', absolute: `${PROJECT}/src/a.rs`, added: 2, removed: 1 }]));
		const tab = reader.open({ ...target, path: 'src/a.rs' });
		await settle();
		expect(tab.operations).toEqual([
			expect.objectContaining({ patch: null, truncated: false, added: 2, removed: 1 }),
		]);
	});

	it('a patch recorded as null stays unavailable', async () => {
		const { reader, items } = setup({ [`${PROJECT}/src/a.rs`]: ['b'] });
		items.push(editItem('e1', [{ path: 'src/a.rs', absolute: `${PROJECT}/src/a.rs`, patch: null }]));
		const tab = reader.open({ ...target, path: 'src/a.rs' });
		await settle();
		expect(tab.operations[0].patch).toBeNull();
	});

	it('a truncated patch is unavailable and flagged, even when some of its text came with it', async () => {
		const { reader, items } = setup({ [`${PROJECT}/src/a.rs`]: ['b'] });
		items.push(
			editItem('e1', [{ path: 'src/a.rs', absolute: `${PROJECT}/src/a.rs`, patch: DIFF_A, patchTruncated: true }]),
		);
		const tab = reader.open({ ...target, path: 'src/a.rs' });
		await settle();
		expect(tab.operations[0]).toMatchObject({ patch: null, truncated: true });
	});

	it('the operations follow the conversation as it grows', async () => {
		const { reader, items } = setup({ [`${PROJECT}/src/a.rs`]: ['b'] });
		const tab = reader.open({ ...target, path: 'src/a.rs' });
		await settle();
		expect(tab.operations).toEqual([]);
		items.push(editItem('e1', [{ path: 'src/a.rs', absolute: `${PROJECT}/src/a.rs`, patch: DIFF_A }]));
		expect(tab.operations.map((op) => op.toolCallId)).toEqual(['tc-e1']);
		items.push(editItem('e2', [{ path: 'src/a.rs', absolute: `${PROJECT}/src/a.rs`, patch: DIFF_B }]));
		expect(tab.operations.map((op) => op.patch)).toEqual([DIFF_A, DIFF_B]);
	});
});
