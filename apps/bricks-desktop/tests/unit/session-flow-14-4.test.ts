// Sprint 14.4, behaviour of « Nouvelle conversation », the single conversation start,
// the per-project drafts and the `/` dispatch wired to the app store. The IPC boundary
// is `invoke` of `@tauri-apps/api/core`: the real client (src/lib/ipc/client.ts) runs
// over it, so every `session_open` / `session_command` the app sends is recorded here.
// Each test gets a fresh app store (modules are reloaded) and fake timers, so the
// debounced preference save never fires between tests.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { runSlashLine, type SlashHost } from '../../src/lib/features/commands/dispatch';
import type { OpenedSession, OpenSessionRequest, Project } from '../../src/lib/ipc/client';
import type { SlashAction } from '../../src/lib/ipc/bindings/SlashAction';
import type { Conversation, DraftAttachment } from '../../src/lib/state/conversation.svelte';
import type { ModelChoice } from '../../src/lib/ipc/engine';
import { MODEL, PROJECT_ID, SESSION_ID, openedSession, snapshot } from './fixtures';

type AppStore = (typeof import('../../src/lib/state/app.svelte'))['app'];

const PROJECT_B = 'proj-test-2';
const MODEL_OTHER = 'prov/other';

const ipc = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => {
	class Channel {
		onmessage: ((event: unknown) => void) | null = null;
	}
	return { Channel, invoke: (cmd: string, args?: unknown) => ipc.invoke(cmd, args) };
});
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }));

// ─── IPC answers ─────────────────────────────────────────────────────────────

let onOpen: (request: OpenSessionRequest) => Promise<OpenedSession>;
let createdCount = 0;

/** A stored, idle session of a project, as `session_open` answers it. */
function stored(id: string, projectId: string, model: string): OpenedSession {
	const base = openedSession();
	return {
		...base,
		session: { ...base.session, id, project_id: projectId },
		snapshot: snapshot({ session_id: id, model })
	};
}

function defaultOpen(request: OpenSessionRequest): Promise<OpenedSession> {
	const id = request.session_id ?? `sess-new-${++createdCount}`;
	return Promise.resolve(stored(id, request.project_id, request.model ?? MODEL));
}

async function answer(cmd: string, args: { request?: OpenSessionRequest } | undefined): Promise<unknown> {
	switch (cmd) {
		case 'session_open':
			return onOpen(args!.request!);
		case 'session_command':
		case 'prefs_set':
			return undefined;
		case 'sessions_list':
		case 'session_agents':
		case 'session_changesets':
			return [];
		default:
			throw new Error(`commande IPC non prévue dans ce test : ${cmd}`);
	}
}

const invokes = (cmd: string) =>
	ipc.invoke.mock.calls.filter(([c]) => c === cmd).map(([, args]) => args as Record<string, unknown>);
const sessionOpens = () => invokes('session_open').map((a) => a.request as OpenSessionRequest);

/** Lets the pending promise chains of the app run to their end. */
async function settle() {
	for (let i = 0; i < 30; i++) await Promise.resolve();
}

function deferred<T>() {
	let resolve!: (value: T) => void;
	const promise = new Promise<T>((r) => (resolve = r));
	return { promise, resolve };
}

function project(id: string, name: string): Project {
	return { id, name, path: `/work/${name.toLowerCase()}`, added_at: 0, last_opened_at: 0, missing: false };
}

const attachment = (name: string): DraftAttachment => ({
	id: `att-${name}`,
	kind: 'file',
	path: `/work/alpha/${name}`,
	name
});

// ─── Fresh app store per test ────────────────────────────────────────────────

let app: AppStore;

beforeEach(async () => {
	vi.useFakeTimers();
	vi.resetModules();
	({ app } = await import('../../src/lib/state/app.svelte'));
	ipc.invoke.mockReset();
	ipc.invoke.mockImplementation(answer);
	onOpen = defaultOpen;
	createdCount = 0;
	app.models = [MODEL, MODEL_OTHER].map((selection) => ({ selection }) as unknown as ModelChoice);
	app.projects = [project(PROJECT_ID, 'Alpha'), project(PROJECT_B, 'Beta')];
	app.currentProjectId = PROJECT_ID;
});

afterEach(() => {
	vi.useRealTimers();
});

// ─── « Nouvelle conversation »: a draft, no session ──────────────────────────

describe('« Nouvelle conversation » opens no session', () => {
	it('20 successive calls without a send: no IPC, one draft, the composer focus moves 20 times', () => {
		const draft = app.draftFor(PROJECT_ID);
		draft.text = 'brouillon';
		const focus = app.composerFocus;

		for (let i = 0; i < 20; i++) app.newConversation();

		expect(ipc.invoke).not.toHaveBeenCalled();
		expect(app.current).toBeNull();
		expect(app.currentSessionId).toBeNull();
		expect(app.draftFor(PROJECT_ID)).toBe(draft);
		expect(draft.text).toBe('brouillon');
		expect(app.composerFocus).toBe(focus + 20);
	});
});

// ─── startConversation: one creation, shared by its entry points ─────────────

describe('startConversation: one session for concurrent entry points', () => {
	it('a composer send and a dispatch submit share one session_open and one conversation', async () => {
		const gate = deferred<OpenedSession>();
		onOpen = () => gate.promise;
		const draft = app.draftFor(PROJECT_ID);
		draft.text = 'Bonjour';
		draft.skills = ['pdf'];
		draft.attachments = [attachment('notes.md')];
		app.setDraftModel(PROJECT_ID, MODEL_OTHER, 'high');

		const fromComposer = app.startConversation(MODEL_OTHER, 'high');
		const fromDispatch = app.startConversation();
		expect(sessionOpens()).toHaveLength(1);

		gate.resolve(stored(SESSION_ID, PROJECT_ID, MODEL_OTHER));
		const [a, b] = await Promise.all([fromComposer, fromDispatch]);

		expect(a).not.toBeNull();
		expect(b).toBe(a);
		expect(app.current).toBe(a);
		expect(sessionOpens()).toEqual([
			{ project_id: PROJECT_ID, session_id: null, model: MODEL_OTHER, reasoning: 'high' }
		]);
	});

	it('the created conversation carries the draft text, attachments, skills, model and effort', async () => {
		const draft = app.draftFor(PROJECT_ID);
		draft.text = 'Bonjour';
		draft.skills = ['pdf'];
		draft.attachments = [attachment('notes.md')];
		app.setDraftModel(PROJECT_ID, MODEL_OTHER, 'high');

		const conv = (await app.startConversation()) as Conversation;

		expect(conv.draft.text).toBe('Bonjour');
		expect(conv.draft.skills).toEqual(['pdf']);
		expect(conv.draft.attachments).toEqual([attachment('notes.md')]);
		expect(conv.model).toBe(MODEL_OTHER);
		expect(sessionOpens()[0]).toEqual({
			project_id: PROJECT_ID,
			session_id: null,
			model: MODEL_OTHER,
			reasoning: 'high'
		});
		// The project draft is kept until the submit is accepted (clearProjectDraft).
		expect(app.draftFor(PROJECT_ID).text).toBe('Bonjour');
	});

	it('a shown conversation of the project is returned: no second session', async () => {
		const first = (await app.startConversation()) as Conversation;
		const again = await app.startConversation();

		expect(again).toBe(first);
		expect(sessionOpens()).toHaveLength(1);
	});

	it('two projects starting at once each get their own session', async () => {
		const first = app.startConversation();
		app.currentProjectId = PROJECT_B;
		const second = app.startConversation();

		await Promise.all([first, second]);

		expect(sessionOpens().map((r) => r.project_id)).toEqual([PROJECT_ID, PROJECT_B]);
	});
});

// ─── Draft isolation between projects ────────────────────────────────────────

describe('drafts are kept per project', () => {
	it('a draft survives a switch to another project and back, an opened conversation and « Nouvelle conversation »', async () => {
		await app.selectProject(PROJECT_ID);
		app.draftFor(PROJECT_ID).text = 'Brouillon A';
		await app.selectProject(PROJECT_B);
		app.draftFor(PROJECT_B).text = 'Brouillon B';
		await app.selectProject(PROJECT_ID);

		const existing = (await app.openSession(SESSION_ID)) as Conversation;
		existing.draft.text = 'Dans la conversation';
		app.newConversation();

		expect(app.current).toBeNull();
		expect(app.draftFor(PROJECT_ID).text).toBe('Brouillon A');
		expect(app.draftFor(PROJECT_B).text).toBe('Brouillon B');
		expect(existing.draft.text).toBe('Dans la conversation');
		expect(await app.openSession(SESSION_ID)).toBe(existing);
		expect(existing.draft.text).toBe('Dans la conversation');
	});

	it('a conversation created in project B takes the draft of B, never the one of A', async () => {
		app.draftFor(PROJECT_ID).text = 'Brouillon A';
		await app.selectProject(PROJECT_B);
		app.draftFor(PROJECT_B).text = 'Brouillon B';

		const conv = (await app.startConversation()) as Conversation;

		expect(conv.projectId).toBe(PROJECT_B);
		expect(conv.draft.text).toBe('Brouillon B');
		expect(sessionOpens()[0].project_id).toBe(PROJECT_B);
	});
});

// ─── `/` dispatch wired to the app store ─────────────────────────────────────

/**
 * The composer's host, with the app store behind it: `ensureSession` is the
 * composer's (the open conversation, or one `startConversation` creates).
 */
function appHost(action: SlashAction) {
	const counts = { ensure: 0 };
	const host: SlashHost = {
		parse: async () => action,
		currentSession: () => app.current?.sessionId ?? null,
		async ensureSession() {
			counts.ensure += 1;
			if (app.current) return { sessionId: app.current.sessionId };
			const conv = await app.startConversation();
			return conv
				? { sessionId: conv.sessionId }
				: { error: app.lastError?.message ?? 'Impossible de créer la conversation.' };
		},
		mark: () => ({ sentAt: 0, runsBefore: 0 }),
		send: (sessionId, command, display) => app.command(sessionId, command, display ?? null),
		async setModel(model, reasoning) {
			app.setDraftModel(PROJECT_ID, model, reasoning);
			return null;
		},
		async setEffort() {
			return null;
		},
		openPicker: () => {},
		open: () => {},
		attachmentBlocks: () => ({ blocks: [] }),
		resume: async () => null,
		showChanges: () => {},
		attach: async () => null,
		quit: async () => null
	};
	return { host, counts };
}

const submitOf = (text: string, skills: string[]): SlashAction => ({
	kind: 'engine',
	command: { type: 'submit', prompt: { blocks: [{ kind: 'text', text }], skills } }
});

describe('dispatch with the app store', () => {
	it('/skill debug fix it with no conversation: one creation, then one submit labelled /skill debug', async () => {
		const { host } = appHost(submitOf('fix it', ['debug']));

		const outcome = await runSlashLine('/skill debug fix it', host);

		expect(outcome).toMatchObject({ kind: 'sent', watch: { kind: 'skill', name: 'debug' } });
		expect(sessionOpens()).toHaveLength(1);
		const sends = invokes('session_command');
		expect(sends).toHaveLength(1);
		expect(sends[0].display).toBe('/skill debug');
		expect(sends[0].command).toEqual({
			type: 'submit',
			prompt: { blocks: [{ kind: 'text', text: 'fix it' }], skills: ['debug'] }
		});
		// The creation comes before the submit.
		const order = ipc.invoke.mock.calls.map(([c]) => c).filter((c) => c === 'session_open' || c === 'session_command');
		expect(order).toEqual(['session_open', 'session_command']);
	});

	it('/skill with a conversation already open: no second session, the submit goes to it with its label', async () => {
		const conv = (await app.startConversation()) as Conversation;
		const { host } = appHost(submitOf('fix it', ['debug']));

		await runSlashLine('/skill debug fix it', host);

		expect(sessionOpens()).toHaveLength(1);
		const sends = invokes('session_command');
		expect(sends).toHaveLength(1);
		expect(sends[0].sessionId).toBe(conv.sessionId);
		expect(sends[0].display).toBe('/skill debug');
	});

	it('/search with no conversation is refused: no session, nothing sent', async () => {
		const { host, counts } = appHost({
			kind: 'engine',
			command: { type: 'search', text: 'x', regex: false }
		});

		const outcome = await runSlashLine('/search x', host);

		expect(outcome).toEqual({ kind: 'refused', message: 'Aucune conversation ouverte.' });
		expect(counts.ensure).toBe(0);
		expect(sessionOpens()).toHaveLength(0);
		expect(invokes('session_command')).toHaveLength(0);
	});

	it('/clear, /compact and /cancel with no conversation are refused without a session', async () => {
		for (const command of [{ type: 'clear_context' }, { type: 'compact' }, { type: 'cancel' }] as const) {
			const { host, counts } = appHost({ kind: 'engine', command });
			const outcome = await runSlashLine(`/${command.type}`, host);
			expect(outcome.kind).toBe('refused');
			expect(counts.ensure).toBe(0);
		}
		expect(sessionOpens()).toHaveLength(0);
		expect(invokes('session_command')).toHaveLength(0);
	});

	it('local views and pickers open without any session', async () => {
		const views = [
			{ kind: 'present', present: { view: 'help' } },
			{ kind: 'present', present: { view: 'context' } },
			{ kind: 'present', present: { view: 'cost' } },
			{ kind: 'present', present: { view: 'session_picker' } },
			{ kind: 'present', present: { view: 'memory' } },
			{ kind: 'present', present: { view: 'config' } },
			{ kind: 'present', present: { view: 'model_picker' } }
		] as SlashAction[];
		for (const action of views) {
			const { host, counts } = appHost(action);
			expect(await runSlashLine('/x', host)).toEqual({ kind: 'done' });
			expect(counts.ensure).toBe(0);
		}
		expect(sessionOpens()).toHaveLength(0);
		expect(ipc.invoke).not.toHaveBeenCalled();
	});

	it('/model with no conversation keeps the choice for the first send and opens no session', async () => {
		const { host } = appHost({
			kind: 'engine',
			command: { type: 'set_model', model: MODEL_OTHER, reasoning: 'high' }
		});

		expect(await runSlashLine('/model prov/other high', host)).toEqual({ kind: 'done' });

		expect(app.draftModel(PROJECT_ID)).toBe(MODEL_OTHER);
		expect(sessionOpens()).toHaveLength(0);
	});
});

// ─── Defects found in review (these fail until the owner fixes the product) ──

describe('defects found in review (expected to fail until fixed)', () => {
	it('a draft typed in an open conversation survives a « lagged » reopen', async () => {
		const conv = (await app.startConversation()) as Conversation;
		conv.draft.text = 'Question non envoyée';

		(app as unknown as { onEvent(e: unknown): void }).onEvent({ kind: 'lagged', session_id: conv.sessionId });
		await settle();

		expect(app.current?.draft.text).toBe('Question non envoyée');
	});

	it('a conversation created for project A is not shown when the user has moved to project B', async () => {
		const gate = deferred<OpenedSession>();
		onOpen = () => gate.promise;

		const pending = app.startConversation();
		await app.selectProject(PROJECT_B);
		gate.resolve(stored(SESSION_ID, PROJECT_ID, MODEL));
		await pending;
		await settle();

		expect(app.currentProjectId).toBe(PROJECT_B);
		expect(app.current).toBeNull();
	});
});
