// A reopened history from before the job tools: its calls to the removed task tools
// (BashTaskStatus, BashTaskOutput, BashTaskStop), and to the later removed tools (EnterWorktree,
// ExitWorktree, SyntheticOutput, ToolSearch), are only tool names now. They render as the
// tools they name, with their results and errors, and they make no user message.
import { describe, expect, it } from 'vitest';
import type { HistoryMessage } from '../../src/lib/ipc/client';
import { Conversation, type ToolItem, type UserItem } from '../../src/lib/state/conversation.svelte';
import { PROJECT_ID, openedSession } from './fixtures';

const history: HistoryMessage[] = [
	{ role: 'user', attachments: [], blocks: [{ type: 'text', text: 'Lance le serveur' }] },
	{
		role: 'assistant',
		attachments: [],
		blocks: [
			{ type: 'tool_use', id: 'old-1', name: 'Bash', input: { command: 'python3 -m http.server', background: true } },
			{ type: 'tool_use', id: 'old-2', name: 'BashTaskOutput', input: { task_id: 'bg-1' } },
			{ type: 'tool_use', id: 'old-3', name: 'BashTaskStatus', input: {} },
			{ type: 'tool_use', id: 'old-4', name: 'BashTaskStop', input: { task_id: 'bg-1' } },
			{ type: 'tool_use', id: 'old-5', name: 'BashTaskOutput', input: { task_id: 'bg-9' } }
		]
	},
	{
		role: 'user',
		attachments: [],
		blocks: [
			{ type: 'tool_result', tool_use_id: 'old-1', content: 'Tâche bg-1 démarrée (pid 42)', is_error: false },
			{ type: 'tool_result', tool_use_id: 'old-2', content: 'Serving HTTP on 127.0.0.1 port 8000', is_error: false },
			{ type: 'tool_result', tool_use_id: 'old-3', content: 'bg-1 [running]', is_error: false },
			{ type: 'tool_result', tool_use_id: 'old-4', content: 'Arrêtée : bg-1 [stopped]', is_error: false },
			{ type: 'tool_result', tool_use_id: 'old-5', content: 'Unknown task bg-9', is_error: true }
		]
	},
	{ role: 'user', attachments: [], blocks: [{ type: 'text', text: 'Merci' }] }
];

function reopen(messages: HistoryMessage[]): Conversation {
	return new Conversation(openedSession({ history: messages }), PROJECT_ID);
}

function tools(conv: Conversation): ToolItem[] {
	return conv.items.flatMap((item) => (item.kind === 'tool' ? [item] : []));
}

describe('reopened history with the removed task tools', () => {
	it('renders each call as the tool it names, with its result', () => {
		const conv = reopen(history);
		const rows = tools(conv);
		expect(rows.map((t) => t.name)).toEqual(['Bash', 'BashTaskOutput', 'BashTaskStatus', 'BashTaskStop', 'BashTaskOutput']);
		expect(rows.map((t) => t.toolCallId)).toEqual(['old-1', 'old-2', 'old-3', 'old-4', 'old-5']);
		expect(rows.map((t) => t.output)).toEqual([
			'Tâche bg-1 démarrée (pid 42)',
			'Serving HTTP on 127.0.0.1 port 8000',
			'bg-1 [running]',
			'Arrêtée : bg-1 [stopped]',
			'Unknown task bg-9',
		]);
		expect(rows.map((t) => t.status)).toEqual(['ok', 'ok', 'ok', 'ok', 'error']);
	});

	it('makes no user message out of the results, only the prompts', () => {
		const conv = reopen(history);
		const users = conv.items.flatMap((item): UserItem[] => (item.kind === 'user' ? [item] : []));
		expect(users.map((u) => u.text)).toEqual(['Lance le serveur', 'Merci']);
	});
});

const laterRemoved: HistoryMessage[] = [
	{ role: 'user', attachments: [], blocks: [{ type: 'text', text: 'Isole le travail' }] },
	{
		role: 'assistant',
		attachments: [],
		blocks: [
			{ type: 'tool_use', id: 'lr-1', name: 'EnterWorktree', input: {} },
			{ type: 'tool_use', id: 'lr-2', name: 'ExitWorktree', input: { action: 'keep' } },
			{ type: 'tool_use', id: 'lr-3', name: 'SyntheticOutput', input: { value: 42 } },
			{ type: 'tool_use', id: 'lr-4', name: 'ToolSearch', input: { query: 'worktree' } }
		]
	},
	{
		role: 'user',
		attachments: [],
		blocks: [
			{ type: 'tool_result', tool_use_id: 'lr-1', content: 'Worktree créé', is_error: false },
			{ type: 'tool_result', tool_use_id: 'lr-2', content: 'Worktree conservé', is_error: false },
			{ type: 'tool_result', tool_use_id: 'lr-3', content: '42', is_error: false },
			{ type: 'tool_result', tool_use_id: 'lr-4', content: 'Unknown tool: ToolSearch', is_error: true }
		]
	},
	{ role: 'user', attachments: [], blocks: [{ type: 'text', text: 'Fini' }] }
];

describe('reopened history with the later removed tools', () => {
	it('renders each call as the tool it names, with its result and error', () => {
		const conv = new Conversation(openedSession({ history: laterRemoved }), PROJECT_ID);
		const rows = tools(conv);
		expect(rows.map((t) => t.name)).toEqual(['EnterWorktree', 'ExitWorktree', 'SyntheticOutput', 'ToolSearch']);
		expect(rows.map((t) => t.toolCallId)).toEqual(['lr-1', 'lr-2', 'lr-3', 'lr-4']);
		expect(rows.map((t) => t.output)).toEqual(['Worktree créé', 'Worktree conservé', '42', 'Unknown tool: ToolSearch']);
		expect(rows.map((t) => t.status)).toEqual(['ok', 'ok', 'ok', 'error']);
	});

	it('makes no user message out of them, only the prompts', () => {
		const conv = new Conversation(openedSession({ history: laterRemoved }), PROJECT_ID);
		const users = conv.items.flatMap((item): UserItem[] => (item.kind === 'user' ? [item] : []));
		expect(users.map((u) => u.text)).toEqual(['Isole le travail', 'Fini']);
	});
});
