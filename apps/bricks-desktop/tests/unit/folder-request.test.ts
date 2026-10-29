// The missing-folder flow of the app store: a project whose folder is gone asks for the folder
// (« Dossier de travail introuvable ») before its conversations open, « Choisir un dossier… »
// relocates the project and runs the waiting action, cancelling keeps the drafts, and an open that
// fails with `folder_missing` asks for the folder too. The IPC boundary is `invoke` of
// `@tauri-apps/api/core` and the native picker is `open` of `@tauri-apps/plugin-dialog`.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { OpenedSession, OpenSessionRequest, Project, SessionItem } from '../../src/lib/ipc/client';
import { openedSession, snapshot } from './fixtures';

type AppStore = (typeof import('../../src/lib/state/app.svelte'))['app'];

const ipc = vi.hoisted(() => ({ invoke: vi.fn() }));
const dialog = vi.hoisted(() => ({ open: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => {
	class Channel {
		onmessage: ((event: unknown) => void) | null = null;
	}
	return { Channel, invoke: (cmd: string, args?: unknown) => ipc.invoke(cmd, args) };
});
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: (options: unknown) => dialog.open(options) }));

const GONE = 'proj-gone';
const OK = 'proj-ok';
const SESSION = 'sess-gone-1';
const MOVED = '/work/moved';

function project(id: string, overrides: Partial<Project> = {}): Project {
	return {
		id,
		name: id,
		path: `/work/${id}`,
		added_at: 0,
		last_opened_at: 0,
		missing: false,
		chats: false,
		...overrides,
	};
}

/** What the desktop answers, per test: the projects on disk, and the refusals to give. */
let onDisk: Project[];
let openError: unknown;
let relocateError: unknown;
let prefs: Record<string, unknown>;

function stored(request: OpenSessionRequest): OpenedSession {
	const base = openedSession();
	return {
		...base,
		session: { ...base.session, id: request.session_id ?? 'sess-new', project_id: request.project_id },
		snapshot: snapshot({ session_id: request.session_id ?? 'sess-new' }),
	};
}

async function answer(cmd: string, args?: Record<string, unknown>): Promise<unknown> {
	switch (cmd) {
		case 'session_open':
			if (openError) throw openError;
			return stored((args as { request: OpenSessionRequest }).request);
		case 'project_relocate': {
			if (relocateError) throw relocateError;
			const { projectId, path } = args as { projectId: string; path: string };
			return project(projectId, { path, missing: false });
		}
		case 'projects_list':
			return onDisk;
		case 'sessions_list':
			return [];
		case 'session_agents':
		case 'session_changesets':
			return [];
		case 'prefs_get':
			return prefs;
		case 'prefs_set':
		case 'events_unsubscribe':
			return undefined;
		case 'events_subscribe':
			return 'sub-1';
		case 'app_info':
			return { version: 'test' };
		case 'models_list':
			return [];
		default:
			throw new Error(`commande IPC non prévue dans ce test : ${cmd}`);
	}
}

const invokes = (cmd: string) =>
	ipc.invoke.mock.calls.filter(([c]) => c === cmd).map(([, args]) => args as Record<string, unknown>);
const sessionOpens = () => invokes('session_open').map((a) => (a.request as OpenSessionRequest).session_id);

let app: AppStore;

beforeEach(async () => {
	vi.useFakeTimers();
	vi.resetModules();
	({ app } = await import('../../src/lib/state/app.svelte'));
	ipc.invoke.mockReset();
	ipc.invoke.mockImplementation(answer);
	dialog.open.mockReset();
	onDisk = [];
	openError = null;
	relocateError = null;
	prefs = { theme: 'system', last_project_id: null, last_session_id: null };
	app.projects = [project(GONE, { missing: true }), project(OK)];
	app.currentProjectId = GONE;
});

afterEach(() => {
	vi.useRealTimers();
});

describe('a project whose folder is missing asks for the folder before it opens', () => {
	it('openSession sets the folder request and opens nothing', async () => {
		const opened = await app.openSession(SESSION);

		expect(opened).toBeNull();
		expect(app.folderRequest).toMatchObject({ projectId: GONE });
		expect(app.folderRequest?.retry).toEqual(expect.any(Function));
		expect(sessionOpens()).toEqual([]);
	});

	it('newConversation of the project shows the request, keeps the current project, and opens nothing', async () => {
		app.currentProjectId = OK;
		await app.newConversation(GONE);

		expect(app.folderRequest).toMatchObject({ projectId: GONE });
		expect(app.currentProjectId).toBe(OK);
		expect(ipc.invoke).not.toHaveBeenCalledWith('session_open', expect.anything());
	});

	it('startConversation keeps the draft of the project and does not open a session', async () => {
		const draft = app.draftFor(GONE);
		draft.text = 'à envoyer plus tard';
		const focus = app.composerFocus;

		expect(await app.startConversation()).toBeNull();

		expect(app.folderRequest).toMatchObject({ projectId: GONE });
		expect(draft.text).toBe('à envoyer plus tard');
		expect(sessionOpens()).toEqual([]);
		expect(app.composerFocus).toBe(focus);
	});

	it('a project that is not missing needs no folder', () => {
		expect(app.requireFolder(OK, null)).toBe(false);
		expect(app.folderRequest).toBeNull();
	});

	it('the built-in project of the conversations never asks for a folder', () => {
		app.projects = [project('chats', { chats: true, missing: true })];
		expect(app.requireFolder('chats', null)).toBe(false);
		expect(app.folderRequest).toBeNull();
	});
});

describe('chooseFolder: the picker, the relocation, then the waiting action', () => {
	beforeEach(async () => {
		await app.openSession(SESSION);
	});

	it('a cancelled picker keeps the request and relocates nothing', async () => {
		const request = app.folderRequest;
		dialog.open.mockResolvedValue(null);

		expect(await app.chooseFolder()).toBe(false);

		expect(app.folderRequest).toBe(request);
		expect(invokes('project_relocate')).toEqual([]);
		expect(sessionOpens()).toEqual([]);
	});

	it('a chosen folder moves the project, clears the request and runs the waiting action', async () => {
		dialog.open.mockResolvedValue(MOVED);

		expect(await app.chooseFolder()).toBe(true);

		expect(invokes('project_relocate')).toEqual([{ projectId: GONE, path: MOVED }]);
		expect(app.projects.find((p) => p.id === GONE)).toMatchObject({ path: MOVED, missing: false });
		expect(app.folderRequest).toBeNull();
		expect(sessionOpens()).toEqual([SESSION]);
		expect(app.current?.sessionId).toBe(SESSION);
	});

	it('the relocated project lists its conversations again', async () => {
		dialog.open.mockResolvedValue(MOVED);
		await app.chooseFolder();

		expect(invokes('sessions_list')).toContainEqual({ projectId: GONE });
	});

	it('a refused relocation shows its message, keeps the request, and runs nothing', async () => {
		dialog.open.mockResolvedValue(MOVED);
		relocateError = { code: 'conflict', message: 'Ce dossier est déjà le projet Autre.' };

		expect(await app.chooseFolder()).toBe(false);

		expect(app.lastError?.message).toBe('Ce dossier est déjà le projet Autre.');
		expect(app.folderRequest).toMatchObject({ projectId: GONE });
		expect(sessionOpens()).toEqual([]);
		expect(app.projects.find((p) => p.id === GONE)?.missing).toBe(true);
	});

	it('without a request there is nothing to choose', async () => {
		app.cancelFolderRequest();
		expect(await app.chooseFolder()).toBe(false);
		expect(dialog.open).not.toHaveBeenCalled();
	});

	it('a new conversation waiting for the folder gets the composer focus back after the relocation', async () => {
		app.cancelFolderRequest();
		const draft = app.draftFor(GONE);
		draft.text = 'brouillon';
		const focus = app.composerFocus;
		await app.startConversation();
		dialog.open.mockResolvedValue(MOVED);

		await app.chooseFolder();

		expect(app.composerFocus).toBe(focus + 1);
		expect(draft.text).toBe('brouillon');
		expect(sessionOpens()).toEqual([]);
	});
});

describe('cancelFolderRequest', () => {
	it('closes the request, and the drafts and the selection stay as they are', async () => {
		await app.openSession(SESSION);
		const draft = app.draftFor(GONE);
		draft.text = 'gardé';

		app.cancelFolderRequest();

		expect(app.folderRequest).toBeNull();
		expect(draft.text).toBe('gardé');
		expect(app.currentProjectId).toBe(GONE);
		expect(ipc.invoke).not.toHaveBeenCalledWith('project_relocate', expect.anything());
	});
});

describe('open() after a folder_missing refusal', () => {
	it('a folder that vanished since the list was loaded asks for it, then opens again once chosen', async () => {
		// The app store still shows the project as present: the desktop refuses the open.
		app.projects = [project(GONE), project(OK)];
		openError = { code: 'folder_missing', message: 'Dossier introuvable' };
		onDisk = [project(GONE, { missing: true }), project(OK)];

		expect(await app.openSession(SESSION)).toBeNull();

		expect(app.folderRequest).toMatchObject({ projectId: GONE });
		expect(app.lastError).toBeNull();
		expect(app.projects.find((p) => p.id === GONE)?.missing).toBe(true);
		expect(sessionOpens()).toEqual([SESSION]);

		// The folder is found and the open is run again.
		openError = null;
		dialog.open.mockResolvedValue(MOVED);
		expect(await app.chooseFolder()).toBe(true);

		expect(sessionOpens()).toEqual([SESSION, SESSION]);
		expect(app.conversations.get(SESSION)?.sessionId).toBe(SESSION);
	});

	it('a folder_missing refusal of a folder still listed as present is an ordinary error', async () => {
		app.projects = [project(GONE), project(OK)];
		openError = { code: 'folder_missing', message: 'Dossier introuvable' };
		onDisk = [project(GONE), project(OK)];

		expect(await app.openSession(SESSION)).toBeNull();

		expect(app.folderRequest).toBeNull();
		expect(app.lastError?.message).toBe('Dossier introuvable');
	});

	it('another refusal is an ordinary error, with no folder request', async () => {
		app.projects = [project(GONE), project(OK)];
		openError = { code: 'io', message: 'disque plein' };

		expect(await app.openSession(SESSION)).toBeNull();

		expect(app.folderRequest).toBeNull();
		expect(app.lastError?.message).toBe('disque plein');
		expect(invokes('projects_list')).toEqual([]);
	});
});

describe('init loads the conversations of a missing project too', () => {
	it('lists the sessions of every project, and does not reselect a missing last project', async () => {
		onDisk = [project(GONE, { missing: true }), project(OK)];
		prefs = { ...prefs, last_project_id: GONE };
		app.currentProjectId = null;
		ipc.invoke.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
			if (cmd === 'sessions_list') {
				const id = (args as { projectId: string }).projectId;
				return id === GONE ? [{ id: 'sess-old', project_id: GONE, title: 'Ancienne' } as unknown as SessionItem] : [];
			}
			return answer(cmd, args);
		});

		await app.init();

		expect(invokes('sessions_list').map((a) => a.projectId).sort()).toEqual([GONE, OK].sort());
		expect(app.sessionsOf(GONE).map((s) => s.id)).toEqual(['sess-old']);
		expect(app.currentProjectId).toBeNull();
		expect(app.ready).toBe(true);
		expect(app.startError).toBeNull();
	});
});
