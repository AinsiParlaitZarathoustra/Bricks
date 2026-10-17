// Sprint 14.4, command labels on user messages, live and after a reopen. Events enter
// through the real subscription (`app.init()` registers the handler on the fake
// Channel), so the routing of `run_labeled` and `engine` events is the one the bridge
// uses. `invoke` of `@tauri-apps/api/core` is the only fake.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type {
	DesktopEvent,
	HistoryMessage,
	OpenedSession,
	OpenSessionRequest,
	Project,
	RunLabel
} from '../../src/lib/ipc/client';
import type { Event as EngineEvent, Snapshot } from '../../src/lib/ipc/engine';
import type { Conversation } from '../../src/lib/state/conversation.svelte';
import { EnvelopeFeed, MODEL, PROJECT_ID, SESSION_ID, T0, openedSession, snapshot } from './fixtures';

type AppModule = typeof import('../../src/lib/state/app.svelte');

const ipc = vi.hoisted(() => ({
	invoke: vi.fn(),
	channel: { current: null as null | { onmessage: ((event: unknown) => void) | null } }
}));
vi.mock('@tauri-apps/api/core', () => {
	class Channel {
		onmessage: ((event: unknown) => void) | null = null;
		constructor() {
			ipc.channel.current = this;
		}
	}
	return { Channel, invoke: (cmd: string, args?: unknown) => ipc.invoke(cmd, args) };
});
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }));

let openAnswer: (request: OpenSessionRequest) => OpenedSession;

async function answer(cmd: string, args: { request?: OpenSessionRequest } | undefined): Promise<unknown> {
	switch (cmd) {
		case 'events_subscribe':
			return 'sub-1';
		case 'events_unsubscribe':
		case 'prefs_set':
			return undefined;
		case 'app_info':
			return {};
		case 'prefs_get': {
			const { DEFAULT_PREFERENCES } = await import('../../src/lib/state/app.svelte');
			return { ...DEFAULT_PREFERENCES, last_project_id: null };
		}
		case 'projects_list':
			return [project];
		case 'models_list':
			return [{ selection: MODEL }];
		case 'session_open':
			return openAnswer(args!.request!);
		case 'sessions_list':
		case 'session_agents':
		case 'session_changesets':
			return [];
		default:
			throw new Error(`commande IPC non prévue dans ce test : ${cmd}`);
	}
}

const project: Project = {
	id: PROJECT_ID,
	name: 'Alpha',
	path: '/work/alpha',
	added_at: T0,
	last_opened_at: T0,
	missing: false
};

type AppStore = AppModule['app'];
let app: AppStore;

beforeEach(async () => {
	vi.useFakeTimers();
	vi.resetModules();
	ipc.invoke.mockReset();
	ipc.invoke.mockImplementation(answer);
	openAnswer = (request) => openedSession({ snapshot: snapshot({ model: request.model ?? MODEL }) });
	({ app } = await import('../../src/lib/state/app.svelte'));
	await app.init();
	app.currentProjectId = PROJECT_ID;
});

afterEach(() => {
	vi.useRealTimers();
});

// ─── Helpers ─────────────────────────────────────────────────────────────────

function emit(event: DesktopEvent) {
	const handler = ipc.channel.current?.onmessage;
	if (!handler) throw new Error('aucun abonnement aux événements');
	handler(event);
}

const engine = (envelope: unknown): DesktopEvent =>
	({ kind: 'engine', project_id: PROJECT_ID, envelope }) as unknown as DesktopEvent;

const labelled = (event: RunLabel, sessionId = SESSION_ID): DesktopEvent =>
	({ kind: 'run_labeled', session_id: sessionId, label: event }) as DesktopEvent;

const label = (runId: string, text: string, prompt: string, userIndex: number | null = null): RunLabel => ({
	run_id: runId,
	label: text,
	prompt,
	at: T0,
	user_index: userIndex
});

const started = (prompt: string): EngineEvent => ({ type: 'run_started', prompt, attachments: [], model: MODEL });

const text = (value: string) => ({ role: 'user', blocks: [{ type: 'text', text: value }] }) as HistoryMessage;
const reply = (value: string) => ({ role: 'assistant', blocks: [{ type: 'text', text: value }] }) as HistoryMessage;

/** The user messages of a conversation, in order: their text and command label. */
function userItems(conv: Conversation): { text: string; commandLabel?: string }[] {
	return conv.items.flatMap((i) => (i.kind === 'user' ? [{ text: i.text, commandLabel: i.commandLabel }] : []));
}

/** Reopens the session with a stored history and labels, as `session_open` answers it. */
function reopen(overrides: Partial<OpenedSession>): Promise<Conversation | null> {
	openAnswer = () => openedSession(overrides);
	return app.openSession(SESSION_ID);
}

// ─── Live: the label and the run, in either order ────────────────────────────

describe('live run labels', () => {
	it('a run_labeled that arrives before its run_started labels the one user message, once', async () => {
		const conv = (await app.startConversation()) as Conversation;
		const feed = new EnvelopeFeed({ runId: 'run-a' });

		emit(labelled(label('run-a', '/skill debug', 'Analyse')));
		emit(engine(feed.next(started('Analyse'))));

		expect(userItems(conv)).toEqual([{ text: 'Analyse', commandLabel: '/skill debug' }]);
	});

	it('a run_started before its run_labeled gets the label; a repeated label changes nothing', async () => {
		const conv = (await app.startConversation()) as Conversation;
		const feed = new EnvelopeFeed({ runId: 'run-a' });
		const event = labelled(label('run-a', '/skill debug', 'Analyse'));

		emit(engine(feed.next(started('Analyse'))));
		emit(event);
		emit(event);

		expect(userItems(conv)).toEqual([{ text: 'Analyse', commandLabel: '/skill debug' }]);
	});

	it('a run started from plain text, or with a skill chosen in the + menu, has no label key at all', async () => {
		const conv = (await app.startConversation()) as Conversation;
		const feed = new EnvelopeFeed({ runId: 'run-a' });

		emit(engine(feed.next(started('Relis ce dossier'))));

		expect(userItems(conv)).toEqual([{ text: 'Relis ce dossier', commandLabel: undefined }]);
		const item = conv.items[0];
		expect(item).not.toHaveProperty('commandLabel');
	});

	it('a label of another session is not applied to this conversation', async () => {
		const conv = (await app.startConversation()) as Conversation;
		const feed = new EnvelopeFeed({ runId: 'run-a' });

		emit(labelled(label('run-a', '/skill debug', 'Analyse'), 'sess-autre'));
		emit(engine(feed.next(started('Analyse'))));

		expect(userItems(conv)).toEqual([{ text: 'Analyse', commandLabel: undefined }]);
	});
});

// ─── Reopened sessions: labels from the stored run_labels ────────────────────

describe('labels of a reopened session', () => {
	it('a label is placed on the one message whose text it names', async () => {
		const conv = await reopen({
			history: [text('Autre'), text('Analyse')],
			run_labels: [label('run-2', '/skill debug', 'Analyse', 1)]
		});

		expect(userItems(conv!)).toEqual([
			{ text: 'Autre', commandLabel: undefined },
			{ text: 'Analyse', commandLabel: '/skill debug' }
		]);
	});

	it('an earlier message with the same text is not taken for the labelled run', async () => {
		// Two messages say « Analyse »: only the second was typed as `/skill debug Analyse`.
		// The stored history carries no run ids, so the label must go to the second one.
		const conv = await reopen({
			history: [text('Analyse'), reply('Réponse 1'), text('Analyse'), reply('Réponse 2')],
			run_labels: [label('run-2', '/skill debug', 'Analyse', 1)]
		});

		expect(userItems(conv!)).toEqual([
			{ text: 'Analyse', commandLabel: undefined },
			{ text: 'Analyse', commandLabel: '/skill debug' }
		]);
	});

	it('a session stored before labels existed opens with no label and no error', async () => {
		const stored = openedSession({ history: [text('Analyse')] });
		delete (stored as { run_labels?: unknown }).run_labels;
		openAnswer = () => stored;

		const conv = (await app.openSession(SESSION_ID)) as Conversation;

		expect(userItems(conv)).toEqual([{ text: 'Analyse', commandLabel: undefined }]);
	});

	it('a label whose run is no longer in the history is dropped, nothing is invented', async () => {
		const conv = await reopen({
			history: [text('Autre')],
			run_labels: [label('run-ghost', '/skill ghost', 'Fantôme')]
		});

		expect(userItems(conv!)).toEqual([{ text: 'Autre', commandLabel: undefined }]);
	});

	it('a run still running is labelled once by its run id, and a repeated label changes nothing', async () => {
		const feed = new EnvelopeFeed({ runId: 'run-a' });
		const running = feed.next(started('Analyse'));
		const conv = await reopen({
			snapshot: snapshot({ activity: { state: 'running', run_id: 'run-a' } as Snapshot['activity'] }),
			recent_events: [running],
			run_labels: [label('run-a', '/skill debug', 'Analyse')]
		});

		expect(userItems(conv!)).toEqual([{ text: 'Analyse', commandLabel: '/skill debug' }]);

		emit(labelled(label('run-a', '/skill debug', 'Analyse')));
		expect(userItems(conv!)).toEqual([{ text: 'Analyse', commandLabel: '/skill debug' }]);
	});
});
