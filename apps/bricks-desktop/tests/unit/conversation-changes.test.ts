// The changes view of a conversation: one count per operation (sub-agent, tool call, path),
// files kept apart by their full path, journal and live timeline counted once together.
import { describe, expect, it } from 'vitest';
import type { ChangeRecord } from '../../src/lib/ipc/client';
import { conversationChanges } from '../../src/lib/features/changes/conversation-changes';
import type { EditItem, TimelineItem } from '../../src/lib/state/conversation.svelte';

const file = (path: string, added: number, removed: number, absolute?: string) => ({
	path,
	kind: 'modify' as const,
	added,
	removed,
	binary: false,
	absolute,
	patch: null,
	patchTruncated: false,
});

function edit(id: string, toolCallId: string, files: EditItem['files'], agentId?: string): EditItem {
	return { kind: 'edit', id, runId: 'run-1', toolCallId, tool: 'Write', files, agentId };
}

function record(id: string, toolCallId: string, files: ChangeRecord['files'], agentId: string | null = null): ChangeRecord {
	return {
		id,
		project_id: 'p',
		session_id: 's',
		run_id: 'run-1',
		agent_id: agentId,
		changeset_id: null,
		tool: 'Write',
		tool_call_id: toolCallId,
		at: 10,
		files,
	};
}

const cf = (path: string, added: number, removed: number, absolute: string | null = null) => ({
	path,
	absolute,
	kind: 'modified',
	added,
	removed,
	binary: false,
	has_patch: true,
});

describe('conversationChanges', () => {
	it('an operation in the journal and in the timeline counts once, from the journal', () => {
		const items: TimelineItem[] = [edit('e1', 'tc-1', [file('a.ts', 2, 1)])];
		const groups = conversationChanges(items, [record('r1', 'tc-1', [cf('a.ts', 2, 1)])], () => 1);
		expect(groups).toHaveLength(1);
		expect(groups[0].files).toHaveLength(1);
		expect(groups[0].files[0]).toMatchObject({ added: 2, removed: 1, operations: [expect.objectContaining({ recordId: 'r1' })] });
	});

	it('the same tool call id of two agents is two operations', () => {
		const items: TimelineItem[] = [
			edit('e1', 'tc-7', [file('a.ts', 1, 0)]),
			edit('e2', 'tc-7', [file('a.ts', 4, 0)], 'ag-2'),
		];
		const [group] = conversationChanges(items, [], () => 1);
		expect(group.files[0]).toMatchObject({ added: 5, operations: expect.any(Array) });
		expect(group.files[0].operations).toHaveLength(2);
		expect(group.files[0].agents).toEqual(['ag-2']);
	});

	it('an operation of one agent listed twice counts once, per path', () => {
		const items: TimelineItem[] = [
			edit('e1', 'tc-1', [file('a.ts', 1, 0), file('b.ts', 3, 0)], 'ag-1'),
			edit('e2', 'tc-1', [file('a.ts', 1, 0)], 'ag-1'),
		];
		const [group] = conversationChanges(items, [], () => 1);
		expect(group.files.map((f) => [f.path, f.added])).toEqual([
			['a.ts', 1],
			['b.ts', 3],
		]);
	});

	it('two files of one name in two folders are two files, kept by their full path', () => {
		const items: TimelineItem[] = [
			edit('e1', 'tc-1', [file('x/mod.rs', 1, 0, '/w/x/mod.rs'), file('y/mod.rs', 2, 0, '/w/y/mod.rs')]),
		];
		const [group] = conversationChanges(items, [], () => 1);
		expect(group.files.map((f) => [f.name, f.key, f.added])).toEqual([
			['mod.rs', '/w/x/mod.rs', 1],
			['mod.rs', '/w/y/mod.rs', 2],
		]);
	});

	it('a file created then deleted is last known as deleted', () => {
		const created = { ...file('tmp.txt', 3, 0, '/w/tmp.txt'), kind: 'create' as const };
		const deleted = { ...file('tmp.txt', 0, 3, '/w/tmp.txt'), kind: 'delete' as const };
		const items: TimelineItem[] = [edit('e1', 'tc-1', [created]), edit('e2', 'tc-2', [deleted])];
		const [group] = conversationChanges(items, [], () => 1);
		expect(group.files[0]).toMatchObject({ kind: 'delete', added: 3, removed: 3 });
	});
});
