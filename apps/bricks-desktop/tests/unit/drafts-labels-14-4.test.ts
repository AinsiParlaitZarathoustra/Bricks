// Unit tests for the Sprint 14.4 application logic: « Nouvelle conversation » without a
// session, per-project drafts, the single conversation start, the command display label
// and its placement on user messages. The IPC client is replaced by a recording fake.
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type {
	DesktopEvent,
	HistoryMessage,
	OpenSessionRequest,
	OpenedSession
} from '../../src/lib/ipc/client';
import type { Command, ModelChoice, Snapshot } from '../../src/lib/ipc/engine';
import { app } from '../../src/lib/state/app.svelte';
import { Conversation } from '../../src/lib/state/conversation.svelte';
import { EnvelopeFeed, MODEL, PROJECT_ID, SESSION_ID, T0, openedSession, snapshot } from './fixtures';

const ipc = vi.hoisted(() => ({
	sessionOpen: vi.fn(),
	sessionCommand: vi.fn(),
	sessionAgents: vi.fn(),
	sessionChangesets: vi.fn(),
	prefsSet: vi.fn(),
	errorMessage: (e: unknown) => (e instanceof Error ? e.message : String(e))
}));
vi.mock('$lib/ipc/client', () => ipc);
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }));

const MODEL_OTHER = 'prov/other';

/** `session_open` answers with the requested model, for the same session id. */
function openAnswer(request: OpenSessionRequest): Promise<OpenedSession> {
	return Promise.resolve(openedSession({ snapshot: snapshot({ model: request.model ?? MODEL }) }));
}

function userLabels(conv: Conversation): (string | undefined)[] {
	return conv.items.flatMap((item) => (item.kind === 'user' ? [item.commandLabel] : []));
}

beforeEach(() => {
	ipc.sessionOpen.mockReset();
	ipc.sessionOpen.mockImplementation(openAnswer);
	ipc.sessionCommand.mockReset();
	ipc.sessionCommand.mockResolvedValue(undefined);
	ipc.sessionAgents.mockReset();
	ipc.sessionAgents.mockResolvedValue([]);
	ipc.sessionChangesets.mockReset();
	ipc.sessionChangesets.mockResolvedValue([]);
	ipc.prefsSet.mockReset();
	ipc.prefsSet.mockResolvedValue(undefined);
	app.conversations.clear();
	app.currentSessionId = null;
	app.currentProjectId = PROJECT_ID;
	app.lastError = null;
	app.models = [MODEL, MODEL_OTHER].map((selection) => ({ selection }) as unknown as ModelChoice);
	app.clearProjectDraft(PROJECT_ID);
});

describe('« Nouvelle conversation »: a draft, no session', () => {
	it('opens no session and keeps the draft, repeated calls included', () => {
		app.draftFor(PROJECT_ID).text = 'brouillon';
		const focus = app.composerFocus;

		app.newConversation();
		app.newConversation();

		expect(ipc.sessionOpen).not.toHaveBeenCalled();
		expect(app.currentSessionId).toBeNull();
		expect(app.draftFor(PROJECT_ID).text).toBe('brouillon');
		expect(app.composerFocus).toBe(focus + 2);
	});

	it('the next conversation takes the model of the one being left', async () => {
		await app.startConversation(MODEL_OTHER);
		app.setDraftModel(PROJECT_ID, MODEL, 'high');

		app.newConversation();

		expect(app.draftModel(PROJECT_ID)).toBe(MODEL_OTHER);
		expect(app.draftReasoning(PROJECT_ID)).toBeNull();
	});

	it('a draft model no longer configured falls back to the first configured model', () => {
		app.setDraftModel(PROJECT_ID, 'gone/x');
		expect(app.draftModel(PROJECT_ID)).toBe(MODEL);
	});
});

describe('startConversation: one creation, the draft hand-over', () => {
	it('concurrent starts for a project share one session_open and one conversation', async () => {
		let release!: (opened: OpenedSession) => void;
		ipc.sessionOpen.mockImplementationOnce(
			() => new Promise<OpenedSession>((resolve) => (release = resolve))
		);

		const first = app.startConversation();
		const second = app.startConversation();
		expect(ipc.sessionOpen).toHaveBeenCalledTimes(1);

		release(openedSession());
		const [a, b] = await Promise.all([first, second]);
		expect(a).not.toBeNull();
		expect(b).toBe(a);
		expect(app.current).toBe(a);

		// Once created, the conversation shown for the project is returned: no second session.
		expect(await app.startConversation()).toBe(a);
		expect(ipc.sessionOpen).toHaveBeenCalledTimes(1);
	});

	it('the draft moves into the conversation; the project draft is cleared only by clearProjectDraft', async () => {
		const draft = app.draftFor(PROJECT_ID);
		draft.text = 'Bonjour';
		draft.skills = ['pdf'];

		const conv = await app.startConversation();
		expect(conv?.draft.text).toBe('Bonjour');
		expect(conv?.draft.skills).toEqual(['pdf']);
		expect(app.draftFor(PROJECT_ID).text).toBe('Bonjour');

		app.clearProjectDraft(PROJECT_ID);
		expect(app.draftFor(PROJECT_ID)).toEqual({ text: '', attachments: [], skills: [] });
		expect(conv?.draft.text).toBe('Bonjour');
	});

	it('a failed session_open keeps the draft and reports the error; a retry can create', async () => {
		ipc.sessionOpen.mockRejectedValueOnce(new Error('Fournisseur indisponible'));
		app.draftFor(PROJECT_ID).text = 'Bonjour';

		expect(await app.startConversation()).toBeNull();
		expect(app.draftFor(PROJECT_ID).text).toBe('Bonjour');
		expect(app.lastError?.message).toBe('Fournisseur indisponible');
		expect(app.current).toBeNull();

		expect(await app.startConversation()).not.toBeNull();
	});

	it('the chosen model and effort are used for the new session', async () => {
		app.setDraftModel(PROJECT_ID, MODEL_OTHER, 'high');

		await app.startConversation();

		expect(ipc.sessionOpen).toHaveBeenCalledWith({
			project_id: PROJECT_ID,
			session_id: null,
			model: MODEL_OTHER,
			reasoning: 'high'
		});
	});
});

describe('commands and run labels', () => {
	it('command passes the typed label to the engine call, null when there is none', async () => {
		const cancel: Command = { type: 'cancel' };

		await app.command('sess-x', cancel, '/skill debug');
		expect(ipc.sessionCommand).toHaveBeenLastCalledWith('sess-x', cancel, '/skill debug');

		await app.command('sess-x', cancel);
		expect(ipc.sessionCommand).toHaveBeenLastCalledWith('sess-x', cancel, null);
	});

	it('a run_labeled event reaches the conversation of its session', async () => {
		const conv = await app.startConversation();
		conv?.apply(new EnvelopeFeed({ runId: 'run-a' }).next({
			type: 'run_started',
			prompt: 'Analyse',
			attachments: [],
			model: MODEL
		}));

		const route = app as unknown as { onEvent(e: DesktopEvent): void };
		route.onEvent({
			kind: 'run_labeled',
			session_id: SESSION_ID,
			label: { run_id: 'run-a', label: '/skill debug', prompt: 'Analyse', at: T0, user_index: null }
		});

		expect(userLabels(conv!)).toEqual(['/skill debug']);
	});
});

describe('command labels on user messages', () => {
	const analyse = { role: 'user', blocks: [{ type: 'text', text: 'Analyse' }] } as HistoryMessage;

	it('a label that arrives before its message is applied when the message is created', () => {
		const conv = new Conversation(openedSession(), PROJECT_ID);
		conv.applyRunLabel({ run_id: 'run-a', label: '/skill debug', prompt: 'Analyse', at: T0, user_index: null });
		conv.apply(
			new EnvelopeFeed({ runId: 'run-a' }).next({
				type: 'run_started',
				prompt: 'Analyse',
				attachments: [],
				model: MODEL
			})
		);
		expect(userLabels(conv)).toEqual(['/skill debug']);
	});

	it('a repeated label leaves a single labelled message', () => {
		const conv = new Conversation(openedSession(), PROJECT_ID);
		conv.apply(
			new EnvelopeFeed({ runId: 'run-a' }).next({
				type: 'run_started',
				prompt: 'Analyse',
				attachments: [],
				model: MODEL
			})
		);
		const label = { run_id: 'run-a', label: '/skill debug', prompt: 'Analyse', at: T0, user_index: null };
		conv.applyRunLabel(label);
		conv.applyRunLabel(label);
		expect(userLabels(conv)).toEqual(['/skill debug']);
	});

	it('a running session replays its live message labelled by run id', () => {
		const running = new EnvelopeFeed({ runId: 'run-a' }).next({
			type: 'run_started',
			prompt: 'Analyse',
			attachments: [],
			model: MODEL
		});
		const conv = new Conversation(
			openedSession({
				snapshot: snapshot({ activity: { state: 'running', run_id: 'run-a' } as Snapshot['activity'] }),
				recent_events: [running],
				run_labels: [{ run_id: 'run-a', label: '/skill debug', prompt: 'Analyse', at: T0, user_index: null }]
			}),
			PROJECT_ID
		);
		expect(userLabels(conv)).toEqual(['/skill debug']);
	});

	it('reopened history: labels match by prompt, in order, at most once, and none is invented', () => {
		const history: HistoryMessage[] = [
			analyse,
			{ role: 'assistant', blocks: [{ type: 'text', text: 'ok' }] },
			analyse,
			{ role: 'user', blocks: [{ type: 'text', text: 'Autre' }] }
		];
		const conv = new Conversation(
			openedSession({
				history,
				run_labels: [
					{ run_id: 'r1', label: '/skill debug', prompt: 'Analyse', at: T0, user_index: null },
					{ run_id: 'r2', label: '/skill review', prompt: 'Analyse', at: T0, user_index: null },
					{ run_id: 'r3', label: '/skill ghost', prompt: 'Fantôme', at: T0, user_index: null }
				]
			}),
			PROJECT_ID
		);
		expect(userLabels(conv)).toEqual(['/skill debug', '/skill review', undefined]);
		expect(userLabels(conv)).not.toContain('/skill ghost');
	});
});

describe('labels by user_index: the index among ALL user-role messages', () => {
	const user = (...blocks: HistoryMessage['blocks']) => ({ role: 'user', blocks }) as HistoryMessage;
	const text = (value: string) => user({ type: 'text', text: value } as HistoryMessage['blocks'][number]);
	const reply = (value: string) =>
		({ role: 'assistant', blocks: [{ type: 'text', text: value }] }) as HistoryMessage;
	const activation = `<skill_activation id="act_1" name="wrangler" source="user" revision="r1">
--- instructions ---
Use the wrangler CLI.
--- end of instructions ---
</skill_activation>`;
	const indexed = (run_id: string, label: string, prompt: string, user_index: number | null) => ({
		run_id,
		label,
		prompt,
		at: T0,
		user_index
	});

	it('the same text twice: the label goes to the message its index names', () => {
		const conv = new Conversation(
			openedSession({
				history: [text('Analyse'), reply('Réponse 1'), text('Analyse'), reply('Réponse 2')],
				run_labels: [indexed('run-2', '/skill debug', 'Analyse', 1)]
			}),
			PROJECT_ID
		);
		expect(userLabels(conv)).toEqual([undefined, '/skill debug']);
	});

	it('a skill activation message counts in the index, though it is not rendered', () => {
		const conv = new Conversation(
			openedSession({
				history: [user({ type: 'text', text: activation } as HistoryMessage['blocks'][number]), text('Analyse')],
				run_labels: [indexed('run-2', '/skill wrangler', 'Analyse', 1)]
			}),
			PROJECT_ID
		);
		// The activation is index 0 and renders nothing: index 1 is the request.
		expect(conv.items.flatMap((i) => (i.kind === 'user' ? [i.text] : []))).toEqual(['Analyse']);
		expect(userLabels(conv)).toEqual(['/skill wrangler']);

		const onActivation = new Conversation(
			openedSession({
				history: [user({ type: 'text', text: activation } as HistoryMessage['blocks'][number]), text('Analyse')],
				run_labels: [indexed('run-3', '/skill ghost', 'Analyse', 0)]
			}),
			PROJECT_ID
		);
		expect(userLabels(onActivation)).toEqual([undefined]);
	});

	it('a user message that makes no user item (tool results) still counts', () => {
		const conv = new Conversation(
			openedSession({
				history: [
					user({ type: 'tool_result', tool_use_id: 't1', content: 'ok', is_error: false } as HistoryMessage['blocks'][number]),
					text('Analyse')
				],
				run_labels: [indexed('run-2', '/skill debug', 'Analyse', 1)]
			}),
			PROJECT_ID
		);
		expect(userLabels(conv)).toEqual(['/skill debug']);
	});

	it('an image-only message is labelled by its index, and its images stay with it', () => {
		const conv = new Conversation(
			openedSession({
				history: [
					text('Avant'),
					user({ type: 'image', path: '/work/a.png', media_type: 'image/png' } as HistoryMessage['blocks'][number])
				],
				run_labels: [indexed('run-2', '/skill debug', '', 1)]
			}),
			PROJECT_ID
		);
		const users = conv.items.flatMap((i) => (i.kind === 'user' ? [i] : []));
		expect(users.map((u) => [u.text, u.attachments.length, u.commandLabel])).toEqual([
			['Avant', 0, undefined],
			['', 1, '/skill debug']
		]);
	});

	it('an index that names no user item is dropped, nothing is invented', () => {
		const conv = new Conversation(
			openedSession({
				history: [text('Analyse')],
				run_labels: [indexed('run-2', '/skill debug', 'Analyse', 7)]
			}),
			PROJECT_ID
		);
		expect(userLabels(conv)).toEqual([undefined]);
	});

	it('a null index (an old label) still matches by its prompt text', () => {
		const conv = new Conversation(
			openedSession({
				history: [text('Autre'), text('Analyse')],
				run_labels: [indexed('run-2', '/skill debug', 'Analyse', null)]
			}),
			PROJECT_ID
		);
		expect(userLabels(conv)).toEqual([undefined, '/skill debug']);
	});
});

describe('optimistic label of a submit: shown as the run starts', () => {
	const submit = { type: 'submit', prompt: { blocks: [{ kind: 'text', text: 'Analyse' }], skills: ['debug'] } } as Command;
	const userMessages = (conv: Conversation) => conv.items.flatMap((i) => (i.kind === 'user' ? [i] : []));
	const emitRunLabel = (conv: Conversation, runId: string, label: string) =>
		(app as unknown as { onEvent(e: unknown): void }).onEvent({
			kind: 'run_labeled',
			session_id: conv.sessionId,
			label: { run_id: runId, label, prompt: 'Analyse', at: T0, user_index: 0 }
		});

	it('the label goes on the message of the run it starts; run_labeled later neither duplicates nor moves it', async () => {
		const conv = (await app.startConversation()) as Conversation;
		expect(await app.command(conv.sessionId, submit, '/skill debug')).toBeNull();

		const feed = new EnvelopeFeed({ runId: 'run-a' });
		conv.apply(feed.next({ type: 'run_started', prompt: 'Analyse', attachments: [], model: MODEL }));
		const [message] = userMessages(conv);
		expect(userLabels(conv)).toEqual(['/skill debug']);

		emitRunLabel(conv, 'run-a', '/skill debug');
		emitRunLabel(conv, 'run-b', '/skill other');

		expect(userMessages(conv).map((m) => m.id)).toEqual([message.id]);
		expect(userLabels(conv)).toEqual(['/skill debug']);
	});

	it('the label belongs to one run: the next run starts without it', async () => {
		const conv = (await app.startConversation()) as Conversation;
		await app.command(conv.sessionId, submit, '/skill debug');
		const feed = new EnvelopeFeed({ runId: 'run-a' });
		conv.apply(feed.next({ type: 'run_started', prompt: 'Analyse', attachments: [], model: MODEL }));
		conv.apply(feed.next({ type: 'run_started', prompt: 'Relis', attachments: [], model: MODEL }, { runId: 'run-b' }));

		expect(userLabels(conv)).toEqual(['/skill debug', undefined]);
	});

	it('a refused submit leaves no label for the run that follows', async () => {
		const conv = (await app.startConversation()) as Conversation;
		ipc.sessionCommand.mockRejectedValueOnce(new Error('Une génération est en cours.'));

		expect(await app.command(conv.sessionId, submit, '/skill debug')).toBe('Une génération est en cours.');
		conv.apply(new EnvelopeFeed({ runId: 'run-a' }).next({ type: 'run_started', prompt: 'Analyse', attachments: [], model: MODEL }));

		expect(userLabels(conv)).toEqual([undefined]);
	});

	it('an engine command_rejected for the submit clears the label', async () => {
		const conv = (await app.startConversation()) as Conversation;
		await app.command(conv.sessionId, submit, '/skill debug');
		const feed = new EnvelopeFeed({ runId: 'run-a' });
		conv.apply(feed.next({ type: 'command_rejected', command: 'submit', reason: 'refusé' }, { runId: null }));
		conv.apply(feed.next({ type: 'run_started', prompt: 'Analyse', attachments: [], model: MODEL }));

		expect(userLabels(conv)).toEqual([undefined]);
	});

	it('a command_rejected of another command does not clear it', async () => {
		const conv = (await app.startConversation()) as Conversation;
		await app.command(conv.sessionId, submit, '/skill debug');
		const feed = new EnvelopeFeed({ runId: 'run-a' });
		conv.apply(feed.next({ type: 'command_rejected', command: 'cancel', reason: 'rien à annuler' }, { runId: null }));
		conv.apply(feed.next({ type: 'run_started', prompt: 'Analyse', attachments: [], model: MODEL }));

		expect(userLabels(conv)).toEqual(['/skill debug']);
	});

	it('a plain submit (no typed command) carries no label', async () => {
		const conv = (await app.startConversation()) as Conversation;
		await app.command(conv.sessionId, submit);
		conv.apply(new EnvelopeFeed({ runId: 'run-a' }).next({ type: 'run_started', prompt: 'Analyse', attachments: [], model: MODEL }));

		expect(userLabels(conv)).toEqual([undefined]);
	});
});
