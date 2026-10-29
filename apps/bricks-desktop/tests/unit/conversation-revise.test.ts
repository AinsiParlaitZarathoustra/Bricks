// Revising a sent message (« Enregistrer et relancer »): what the conversation state does with
// `history_revised` (the message and what follows leave the timeline, with their runs), what
// `revisedBlocks` and `userIndexOf` answer, and what `app.reviseMessage` refuses or sends.
// The IPC boundary is `invoke` of `@tauri-apps/api/core`; the app store is loaded fresh per test.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { HistoryMessage, OpenedSession, RunRecord } from '../../src/lib/ipc/client';
import type { Envelope } from '../../src/lib/ipc/engine';
import { Conversation, type UserItem } from '../../src/lib/state/conversation.svelte';
import { EnvelopeFeed, PROJECT_ID, SESSION_ID, T0, openedSession } from './fixtures';

type AppStore = (typeof import('../../src/lib/state/app.svelte'))['app'];

const ipc = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => {
	class Channel {
		onmessage: ((event: unknown) => void) | null = null;
	}
	return { Channel, invoke: (cmd: string, args?: unknown) => ipc.invoke(cmd, args) };
});
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }));

const RUN_1 = 'run-1';
const RUN_2 = 'run-2';
const RUN_3 = 'run-3';

const user = (text: string): HistoryMessage => ({ role: 'user', attachments: [], blocks: [{ type: 'text', text }] });
const assistant = (text: string): HistoryMessage => ({ role: 'assistant', attachments: [], blocks: [{ type: 'text', text }] });

function start(conv: Conversation, feed: EnvelopeFeed, prompt: string, runId: string) {
	conv.apply(feed.next({ type: 'run_started', prompt, attachments: [], model: 'm' }, { runId, at: T0 }));
}

function answer(conv: Conversation, feed: EnvelopeFeed, text: string, runId: string) {
	conv.apply(feed.next({ type: 'text_delta', text }, { runId }));
	conv.apply(feed.next({ type: 'run_finished', outcome: 'succeeded', text, turns: 1 }, { runId }));
}

/** Two runs of a live conversation: « Première » then « Seconde », each answered. */
function twoLiveRuns(): { conv: Conversation; feed: EnvelopeFeed } {
	const conv = new Conversation(openedSession(), PROJECT_ID);
	const feed = new EnvelopeFeed({ runId: RUN_1 });
	start(conv, feed, 'Première', RUN_1);
	answer(conv, feed, 'Réponse 1', RUN_1);
	start(conv, feed, 'Seconde', RUN_2);
	answer(conv, feed, 'Réponse 2', RUN_2);
	return { conv, feed };
}

/** A stored conversation of three exchanges: A, B and C, each answered. */
function storedThree(extra: Partial<OpenedSession> = {}): Conversation {
	return new Conversation(
		openedSession({
			history: [user('A'), assistant('a'), user('B'), assistant('b'), user('C'), assistant('c')],
			...extra,
		}),
		PROJECT_ID,
	);
}

function userItem(conv: Conversation, text: string): UserItem {
	const item = conv.items.find((i): i is UserItem => i.kind === 'user' && i.text === text);
	if (!item) throw new Error(`no user item « ${text} »`);
	return item;
}

function revised(feed: EnvelopeFeed, userIndex: number, messagesRemoved = 2): Envelope {
	return feed.next({ type: 'history_revised', user_index: userIndex, messages_removed: messagesRemoved }, { runId: null });
}

const texts = (conv: Conversation) => conv.items.map((i) => (i.kind === 'user' || i.kind === 'assistant' ? i.text : i.kind));

// ─── What history_revised removes ───────────────────────────────────────────

describe('history_revised: the revised message and what follows leave the timeline', () => {
	it('a live conversation revised at its first message is emptied, with its runs', () => {
		const { conv, feed } = twoLiveRuns();
		conv.revisingItemId = userItem(conv, 'Première').id;
		conv.apply(revised(feed, 0, 4));

		expect(conv.items).toEqual([]);
		expect(conv.runs.has(RUN_1)).toBe(false);
		expect(conv.runs.has(RUN_2)).toBe(false);
		expect(conv.responses).toEqual([]);
		expect(conv.revisingItemId).toBeNull();
	});

	it('a live conversation keeps the messages before the revised one and their runs', () => {
		const { conv, feed } = twoLiveRuns();
		start(conv, feed, 'Troisième', RUN_3);
		answer(conv, feed, 'Réponse 3', RUN_3);
		conv.revisingItemId = userItem(conv, 'Seconde').id;
		conv.apply(revised(feed, 1));

		expect(texts(conv)).toEqual(['Première', 'Réponse 1', 'run_end']);
		expect(conv.runs.has(RUN_1)).toBe(true);
		expect(conv.runs.has(RUN_2)).toBe(false);
		expect(conv.runs.has(RUN_3)).toBe(false);
	});

	it('the new run of the revision follows the cut, as a new user message', () => {
		const { conv, feed } = twoLiveRuns();
		conv.revisingItemId = userItem(conv, 'Seconde').id;
		conv.apply(revised(feed, 1));
		start(conv, feed, 'Seconde, corrigée', 'run-4');

		expect(conv.items.filter((i) => i.kind === 'user').map((i) => i.kind === 'user' && i.text)).toEqual([
			'Première',
			'Seconde, corrigée',
		]);
		expect(conv.running).toBe(true);
		expect(conv.runningRunId).toBe('run-4');
	});

	it('a stored conversation revised by its user index (no pending item) cuts from that message', () => {
		const conv = storedThree();
		const feed = new EnvelopeFeed({ runId: 'x' });
		conv.apply(revised(feed, 1));

		expect(texts(conv)).toEqual(['A', 'a']);
		expect(conv.responses.map((g) => g.copyText)).toEqual(['a']);
	});

	it('a stored conversation drops the removed messages from the index of user messages', async () => {
		const conv = storedThree();
		const feed = new EnvelopeFeed({ runId: 'x' });
		const removed = userItem(conv, 'C');
		conv.apply(revised(feed, 1));

		expect(await conv.userIndexOf(userItem(conv, 'A'))).toBe(0);
		// C was cut: it is no longer in the timeline, and its index is gone with it.
		expect(await conv.userIndexOf(removed)).toBeNull();
		expect(ipc.invoke).not.toHaveBeenCalledWith('session_run_user_index', expect.anything());
	});

	it('a stored run of a cut message leaves with its record', () => {
		const record: RunRecord = {
			version: 1,
			run_id: 'stored-c',
			user_index: 2,
			started_at: T0,
			finished_at: T0 + 1_000,
			outcome: 'succeeded',
			files: [],
			commands: [],
			assistant_text: 'Texte de C',
			usage: null,
		};
		const conv = storedThree({ run_records: [record] });
		expect(conv.run('stored-c')).toBeDefined();

		const feed = new EnvelopeFeed({ runId: 'x' });
		conv.apply(revised(feed, 1));

		expect(conv.run('stored-c')).toBeUndefined();
		expect(conv.responses.map((g) => g.copyText)).toEqual(['a']);
	});

	it('an index that matches no message removes nothing', () => {
		const conv = storedThree();
		const feed = new EnvelopeFeed({ runId: 'x' });
		const before = texts(conv);
		conv.apply(revised(feed, 9));
		expect(texts(conv)).toEqual(before);
	});

	it('a refused revision (command_rejected) clears the pending item and keeps the message', () => {
		const { conv, feed } = twoLiveRuns();
		const item = userItem(conv, 'Seconde');
		conv.revisingItemId = item.id;
		conv.apply(feed.next({ type: 'command_rejected', command: 'revise', reason: 'désaccord' }, { runId: null }));

		expect(conv.revisingItemId).toBeNull();
		expect(userItem(conv, 'Seconde').id).toBe(item.id);
		expect(conv.runs.has(RUN_2)).toBe(true);
	});
});

// ─── userIndexOf and revisedBlocks ──────────────────────────────────────────

describe('userIndexOf and revisedBlocks', () => {
	beforeEach(() => {
		ipc.invoke.mockReset();
	});

	it('a stored message has the index it was loaded with, without any IPC call', async () => {
		const conv = storedThree();
		expect(await conv.userIndexOf(userItem(conv, 'B'))).toBe(1);
		expect(ipc.invoke).not.toHaveBeenCalled();
	});

	it('a message sent in this view asks the engine for its index, by run', async () => {
		const { conv } = twoLiveRuns();
		ipc.invoke.mockResolvedValue(1);
		expect(await conv.userIndexOf(userItem(conv, 'Seconde'))).toBe(1);
		expect(ipc.invoke).toHaveBeenCalledWith('session_run_user_index', { sessionId: SESSION_ID, runId: RUN_2 });
	});

	it('a message with no run and no stored index has no index, and asks the engine nothing', async () => {
		const conv = new Conversation(openedSession({ history: [] }), PROJECT_ID);
		const orphan: UserItem = { kind: 'user', id: 'u-orphan', text: 'Perdu', attachments: [] };
		expect(await conv.userIndexOf(orphan)).toBeNull();
		expect(ipc.invoke).not.toHaveBeenCalled();
	});

	it('the blocks of a revision are the trimmed text, then the message attachments as copies', async () => {
		const conv = new Conversation(openedSession({ history: [] }), PROJECT_ID);
		const item: UserItem = {
			kind: 'user',
			id: 'u-att',
			text: 'Avant',
			attachments: [
				{ kind: 'image', path: '/src/shot.png', bytes: 3, attachment_id: 'att-1' },
				{ kind: 'folder', path: '/src/docs', bytes: 0 },
				{ kind: 'file', path: '/src/a.md', bytes: 9 },
			],
		};
		ipc.invoke.mockResolvedValue([{ id: 'att-1', kind: 'image', path: '/copies/shot.png' }]);

		const blocks = await conv.revisedBlocks(item, '  Après  ');
		expect(blocks).toEqual([
			{ kind: 'text', text: 'Après' },
			{ kind: 'image', path: '/copies/shot.png' },
			{ kind: 'folder', path: '/src/docs' },
			{ kind: 'file', path: '/src/a.md' },
		]);
	});

	it('a blank text sends no text block, and a copy that cannot be found fails the revision', async () => {
		const conv = new Conversation(openedSession({ history: [] }), PROJECT_ID);
		const item: UserItem = {
			kind: 'user',
			id: 'u-lost',
			text: '',
			attachments: [{ kind: 'file', path: '/src/gone.md', bytes: 1, attachment_id: 'att-gone' }],
		};
		ipc.invoke.mockResolvedValue([]);
		await expect(conv.revisedBlocks(item, '   ')).rejects.toThrow('La copie de « /src/gone.md » est introuvable.');
	});
});

// ─── app.reviseMessage: refusals and the command sent ───────────────────────

describe('app.reviseMessage', () => {
	let app: AppStore;

	beforeEach(async () => {
		vi.useFakeTimers();
		vi.resetModules();
		({ app } = await import('../../src/lib/state/app.svelte'));
		ipc.invoke.mockReset();
		ipc.invoke.mockImplementation(async (cmd: string) => {
			if (cmd === 'session_command') return undefined;
			if (cmd === 'session_run_user_index') return null;
			if (cmd === 'session_attachment_paths') return [];
			throw new Error(`commande IPC non prévue : ${cmd}`);
		});
	});

	afterEach(() => {
		vi.useRealTimers();
	});

	const commands = () =>
		ipc.invoke.mock.calls.filter(([cmd]) => cmd === 'session_command').map(([, args]) => args as Record<string, unknown>);

	it('sends the revise command with the index, the original text and the new prompt', async () => {
		const conv = storedThree();
		const error = await app.reviseMessage(conv, userItem(conv, 'B'), 'B corrigé');

		expect(error).toBeNull();
		expect(commands()).toEqual([
			{
				sessionId: SESSION_ID,
				command: {
					type: 'revise',
					user_index: 1,
					original: 'B',
					prompt: { blocks: [{ kind: 'text', text: 'B corrigé' }], ask_timer: { mode: 'off' } },
				},
				display: null,
			},
		]);
		expect(conv.revisingItemId).toBe(userItem(conv, 'B').id);
	});

	it('a message of this view is revised by the index the engine gives its run', async () => {
		const { conv } = twoLiveRuns();
		ipc.invoke.mockImplementation(async (cmd: string) => (cmd === 'session_run_user_index' ? 1 : undefined));
		const error = await app.reviseMessage(conv, userItem(conv, 'Seconde'), 'Seconde bis');

		expect(error).toBeNull();
		expect(commands()[0]).toMatchObject({ command: { type: 'revise', user_index: 1, original: 'Seconde' } });
	});

	it('refused while a run is going, and nothing is sent', async () => {
		const conv = new Conversation(
			openedSession({ snapshot: { ...openedSession().snapshot, activity: { state: 'running', run_id: RUN_1 } } }),
			PROJECT_ID,
		);
		const error = await app.reviseMessage(conv, { kind: 'user', id: 'u', text: 'x', attachments: [] }, 'y');

		expect(error).toBe('Une exécution est en cours : attendez sa fin ou annulez-la.');
		expect(ipc.invoke).not.toHaveBeenCalled();
	});

	it('refused when the index of the message is unknown, and nothing is sent', async () => {
		const conv = new Conversation(openedSession({ history: [] }), PROJECT_ID);
		const orphan: UserItem = { kind: 'user', id: 'u-orphan', text: 'Perdu', attachments: [] };
		const error = await app.reviseMessage(conv, orphan, 'Nouveau');

		expect(error).toBe('Ce message ne peut pas être modifié : rouvrez la conversation.');
		expect(commands()).toEqual([]);
		expect(conv.revisingItemId).toBeNull();
	});

	it('refused when the engine does not know the run of the message', async () => {
		const { conv } = twoLiveRuns();
		const error = await app.reviseMessage(conv, userItem(conv, 'Seconde'), 'Nouveau');

		expect(error).toBe('Ce message ne peut pas être modifié : rouvrez la conversation.');
		expect(commands()).toEqual([]);
	});

	it('an empty message with no attachment is refused', async () => {
		const conv = storedThree();
		const error = await app.reviseMessage(conv, userItem(conv, 'B'), '   ');

		expect(error).toBe('Le message est vide.');
		expect(commands()).toEqual([]);
	});

	it('an engine refusal is returned and the message is not left pending', async () => {
		const conv = storedThree();
		ipc.invoke.mockImplementation(async (cmd: string) => {
			if (cmd === 'session_command') throw { code: 'session_busy', message: 'session occupée' };
			return undefined;
		});
		const error = await app.reviseMessage(conv, userItem(conv, 'B'), 'Nouveau');

		expect(error).toBe('session occupée');
		expect(conv.revisingItemId).toBeNull();
	});
});
