// A reopened history: a turn of tool results is the engine's, so the text and the images in it
// (a tool's image label, its picture, the engine's notes) make no user item. The prompt's own
// message, with its attachments, still does. The ViewImage call keeps its own item.
import { describe, expect, it } from 'vitest';
import type { HistoryMessage } from '../../src/lib/ipc/client';
import { Conversation, type ToolItem, type UserItem } from '../../src/lib/state/conversation.svelte';
import { PROJECT_ID, openedSession } from './fixtures';

const CALL = 'toolu_view_1';
const LABEL = `[image from tool call ${CALL}] /work/shot.png, image/png, 70 bytes`;

const history: HistoryMessage[] = [
	{
		role: 'user',
		attachments: [],
		blocks: [{ type: 'text', text: 'Regarde cette image' }]
	},
	{
		role: 'assistant',
		attachments: [],
		blocks: [{ type: 'tool_use', id: CALL, name: 'ViewImage', input: { file_path: '/work/shot.png' } }]
	},
	{
		// The engine's turn of results: the text of the result, the label, the picture, and a note.
		role: 'user',
		attachments: [],
		blocks: [
			{ type: 'tool_result', tool_use_id: CALL, content: 'Image /work/shot.png: image/png, 1×1, 70 bytes.', is_error: false },
			{ type: 'text', text: LABEL },
			{ type: 'image', path: null, media_type: 'image/png' },
			{ type: 'text', text: '[system] Your last 3 rounds repeated earlier ones.' }
		]
	},
	{
		role: 'assistant',
		attachments: [],
		blocks: [{ type: 'text', text: 'Un petit carré rouge.' }]
	},
	{
		role: 'user',
		attachments: [],
		blocks: [{ type: 'text', text: 'Merci' }]
	}
];

function reopen(messages: HistoryMessage[]): Conversation {
	return new Conversation(openedSession({ history: messages }), PROJECT_ID);
}

function userItems(conv: Conversation): UserItem[] {
	return conv.items.flatMap((item) => (item.kind === 'user' ? [item] : []));
}

describe('reopened history: a turn of tool results is not a user message', () => {
	it('creates no user item for the label, the image and the notes of the engine turn', () => {
		const conv = reopen(history);
		expect(userItems(conv).map((u) => u.text)).toEqual(['Regarde cette image', 'Merci']);
		const texts = conv.items.flatMap((item) => (item.kind === 'user' ? [item.text] : []));
		expect(texts.some((t) => t.includes('[image from tool call'))).toBe(false);
		expect(texts.some((t) => t.includes('[system]'))).toBe(false);
	});

	it('the prompt of the run still makes its user item, with its own attachments', () => {
		const conv = reopen([
			{
				role: 'user',
				attachments: [],
				blocks: [
					{ type: 'text', text: 'Voici' },
					{ type: 'image', path: '/work/pic.png', media_type: 'image/png' }
				]
			}
		]);
		const [item] = userItems(conv);
		expect(item.text).toBe('Voici');
		expect(item.attachments).toEqual([{ kind: 'image', path: '/work/pic.png', bytes: 0 }]);
	});

	it('the ViewImage call keeps its own item, finished and not a user message', () => {
		const conv = reopen(history);
		const tool = conv.items.find((i): i is ToolItem => i.kind === 'tool');
		expect(tool).toBeDefined();
		expect(tool?.name).toBe('ViewImage');
		expect(tool?.toolCallId).toBe(CALL);
		expect(tool?.status).toBe('ok');
		expect(tool?.output).toBe('Image /work/shot.png: image/png, 1×1, 70 bytes.');
	});

	it('an engine turn whose call failed marks the call as an error and still no user item', () => {
		const conv = reopen([
			{
				role: 'assistant',
				attachments: [],
				blocks: [{ type: 'tool_use', id: 'v-bad', name: 'ViewImage', input: { file_path: 'x.png' } }]
			},
			{
				role: 'user',
				attachments: [],
				blocks: [
					{ type: 'tool_result', tool_use_id: 'v-bad', content: 'x.png: not an image', is_error: true },
					{ type: 'text', text: '[image from tool call v-bad] ignored' }
				]
			}
		]);
		expect(userItems(conv)).toEqual([]);
		const tool = conv.items.find((i): i is ToolItem => i.kind === 'tool');
		expect(tool?.status).toBe('error');
	});
});
