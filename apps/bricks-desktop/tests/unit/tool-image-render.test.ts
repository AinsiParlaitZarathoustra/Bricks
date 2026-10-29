// @vitest-environment happy-dom
// The picture of a ViewImage call (ToolImage.svelte, under ToolRow.svelte): mounted in happy-dom
// with the IPC client replaced by a fake. A finished call with status ok asks the session for the
// image it showed and shows it when there is one; nothing else asks, and nothing is shown when the
// session has none. The question is asked again as the timeline grows, until an image is found.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import ToolRow from '../../src/lib/features/conversation/timeline/ToolRow.svelte';
import { Conversation, type ToolItem } from '../../src/lib/state/conversation.svelte';
import type { HistoryMessage } from '../../src/lib/ipc/client';
import { EnvelopeFeed, PROJECT_ID, SESSION_ID, openedSession } from './fixtures';

const ipc = vi.hoisted(() => ({ sessionToolImage: vi.fn() }));
vi.mock('$lib/ipc/client', async (importOriginal) => ({
	...(await importOriginal<typeof import('../../src/lib/ipc/client')>()),
	sessionToolImage: ipc.sessionToolImage
}));

const PNG_URL = 'data:image/png;base64,QUJD';
const CALL = 'toolu_show_1';

/** A reopened conversation whose one tool call is `name`, finished with `isError`. */
function reopened(name: string, isError = false): Conversation {
	const history: HistoryMessage[] = [
		{
			role: 'assistant',
			attachments: [],
			blocks: [{ type: 'tool_use', id: CALL, name, input: { file_path: '/work/shot.png' } }]
		},
		{
			role: 'user',
			attachments: [],
			blocks: [{ type: 'tool_result', tool_use_id: CALL, content: 'ok', is_error: isError }]
		}
	];
	return new Conversation(openedSession({ history }), PROJECT_ID);
}

function toolOf(conv: Conversation): ToolItem {
	const tool = conv.items.find((i): i is ToolItem => i.kind === 'tool');
	if (!tool) throw new Error('no tool item');
	return tool;
}

let target: HTMLDivElement;
let app: ReturnType<typeof mount> | null = null;

function show(conv: Conversation): void {
	app = mount(ToolRow, { target, props: { item: toolOf(conv), conversation: conv } });
	flushSync();
}

/** Lets the pending IPC answers settle, then renders what they changed. */
async function settle(): Promise<void> {
	await new Promise((resolve) => setTimeout(resolve, 0));
	flushSync();
}

function picture(): HTMLImageElement | null {
	return target.querySelector('img');
}

beforeEach(() => {
	ipc.sessionToolImage.mockReset();
	target = document.createElement('div');
	document.body.appendChild(target);
});

afterEach(() => {
	if (app) void unmount(app);
	app = null;
	target.remove();
});

describe('ViewImage: the picture the model was shown', () => {
	it('shows the image the session returns for the call, named by its file', async () => {
		ipc.sessionToolImage.mockResolvedValue(PNG_URL);
		show(reopened('ViewImage'));
		await vi.waitFor(() => expect(picture()).not.toBeNull());
		const img = picture()!;
		expect(img.getAttribute('src')).toBe(PNG_URL);
		expect(img.getAttribute('alt')).toBe('shot.png');
		expect(img.getAttribute('title')).toBe('/work/shot.png');
		expect(ipc.sessionToolImage).toHaveBeenCalledWith(SESSION_ID, CALL);
	});

	it('shows nothing when the session has no image for the call', async () => {
		ipc.sessionToolImage.mockResolvedValue(null);
		show(reopened('ViewImage'));
		await vi.waitFor(() => expect(ipc.sessionToolImage).toHaveBeenCalledTimes(1));
		await settle();
		expect(picture()).toBeNull();
	});

	it('shows nothing and does not fail when the lookup fails', async () => {
		ipc.sessionToolImage.mockRejectedValue(new Error('session fermée'));
		show(reopened('ViewImage'));
		await vi.waitFor(() => expect(ipc.sessionToolImage).toHaveBeenCalledTimes(1));
		await settle();
		expect(picture()).toBeNull();
	});

	it('asks nothing for a ViewImage call that failed', async () => {
		ipc.sessionToolImage.mockResolvedValue(PNG_URL);
		show(reopened('ViewImage', true));
		await settle();
		expect(ipc.sessionToolImage).not.toHaveBeenCalled();
		expect(picture()).toBeNull();
	});

	it('other tools never ask for an image, whatever their output', async () => {
		ipc.sessionToolImage.mockResolvedValue(PNG_URL);
		for (const name of ['Read', 'Bash', 'WebFetch']) {
			show(reopened(name));
			await settle();
			expect(picture(), name).toBeNull();
			unmountCurrent();
		}
		expect(ipc.sessionToolImage).not.toHaveBeenCalled();
	});

	it('a call still running asks nothing, and asks once it has finished ok', async () => {
		ipc.sessionToolImage.mockResolvedValue(PNG_URL);
		const conv = new Conversation(openedSession(), PROJECT_ID);
		const feed = new EnvelopeFeed({ runId: 'run-view' });
		conv.apply(feed.next({ type: 'run_started', prompt: 'Regarde', attachments: [], model: 'm' }));
		conv.apply(
			feed.next({ type: 'tool_started', tool_call_id: CALL, name: 'ViewImage', input: { file_path: '/work/shot.png' } })
		);
		show(conv);
		await settle();
		expect(ipc.sessionToolImage).not.toHaveBeenCalled();
		expect(picture()).toBeNull();

		conv.apply(
			feed.next({
				type: 'tool_finished',
				tool_call_id: CALL,
				name: 'ViewImage',
				is_error: false,
				duration_ms: 12,
				output: 'Image /work/shot.png: image/png, 1×1, 70 bytes.',
				executed: true,
				status: 'success'
			})
		);
		flushSync();
		await vi.waitFor(() => expect(picture()).not.toBeNull());
		expect(picture()!.getAttribute('src')).toBe(PNG_URL);
		expect(ipc.sessionToolImage).toHaveBeenCalledTimes(1);
	});

	it('asks again as the timeline grows, until the session has the image', async () => {
		ipc.sessionToolImage.mockResolvedValueOnce(null).mockResolvedValueOnce(PNG_URL);
		const conv = reopened('ViewImage');
		show(conv);
		await vi.waitFor(() => expect(ipc.sessionToolImage).toHaveBeenCalledTimes(1));
		await settle();
		expect(picture()).toBeNull();

		const feed = new EnvelopeFeed({ runId: 'run-later' });
		conv.apply(feed.next({ type: 'run_started', prompt: 'Encore', attachments: [], model: 'm' }));
		flushSync();
		await vi.waitFor(() => expect(picture()).not.toBeNull());
		expect(ipc.sessionToolImage).toHaveBeenCalledTimes(2);
	});
});

/** Unmounts the rendered row between two renders in one test. */
function unmountCurrent(): void {
	if (app) void unmount(app);
	app = null;
	target.innerHTML = '';
}
