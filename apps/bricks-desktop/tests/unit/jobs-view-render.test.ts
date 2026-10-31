// @vitest-environment happy-dom
// « Tâches de fond » (JobsView.svelte), mounted over the app store with a real Conversation: the
// section is folded by default and its summary states the jobs in one line; each job opens on
// demand; « Arrêter » is offered for a running job only; the refresh says it is pending, then reads
// the empty list or the engine's refusal.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import JobsView from '../../src/lib/features/catalog/JobsView.svelte';
import { app } from '../../src/lib/state/app.svelte';
import { Conversation, type JobInfo } from '../../src/lib/state/conversation.svelte';
import { PROJECT_ID, SESSION_ID, T0, openedSession } from './fixtures';

const ipc = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => {
	class Channel {
		onmessage: ((event: unknown) => void) | null = null;
	}
	return { Channel, invoke: (cmd: string, args?: unknown) => ipc.invoke(cmd, args) };
});
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }));

let target: HTMLDivElement;
let component: ReturnType<typeof mount> | null = null;

beforeEach(() => {
	ipc.invoke.mockReset();
	ipc.invoke.mockResolvedValue(undefined);
	app.conversations.set(SESSION_ID, new Conversation(openedSession(), PROJECT_ID));
	app.currentSessionId = SESSION_ID;
	target = document.createElement('div');
	document.body.appendChild(target);
});

afterEach(() => {
	if (component) void unmount(component);
	component = null;
	target.remove();
	app.conversations.clear();
	app.currentSessionId = null;
});

function current(): Conversation {
	return app.current!;
}

function job(jobId: string, state: string, command: string): JobInfo {
	return {
		jobId,
		agentId: 'agent-1',
		rootRunId: 'run-1',
		command,
		cwd: '/work/demo',
		pid: 4242,
		startedAt: T0,
		stdoutBytes: 0,
		stderrBytes: 0,
		state,
		logs: []
	};
}

function render() {
	component = mount(JobsView, { target, props: { args: {} } });
	flushSync();
}

const section = () => target.querySelector<HTMLElement>('section[aria-label="Tâches de fond"]')!;
const trigger = () => section().querySelector<HTMLButtonElement>('button')!;
const summary = () => target.querySelector<HTMLElement>('section[aria-label="Tâches de fond"] button')?.textContent?.trim();
const rows = () => [...section().querySelectorAll<HTMLElement>('ul > li')];
/** The body of the section: bits-ui keeps it in the DOM and marks it `hidden` while folded. */
const body = () => section().querySelector<HTMLElement>('[data-collapsible-content]')!;

function open() {
	trigger().click();
	flushSync();
}

describe('folded by default', () => {
	it('shows the summary of the states, and keeps the jobs hidden until the section opens', () => {
		current().jobs.set('j1', job('j1', 'running', 'npm run dev'));
		current().jobs.set('j2', job('j2', 'completed', 'npm test'));
		current().jobs.set('j3', job('j3', 'completed', 'npm run build'));
		render();
		expect(summary()).toBe('1 en cours · 2 terminées');
		expect(trigger().getAttribute('aria-expanded')).toBe('false');
		expect(body().hasAttribute('hidden')).toBe(true);
	});

	it('opens on click, with one row per job', () => {
		current().jobs.set('j1', job('j1', 'running', 'npm run dev'));
		current().jobs.set('j2', job('j2', 'completed', 'npm test'));
		render();
		open();
		expect(trigger().getAttribute('aria-expanded')).toBe('true');
		expect(body().hasAttribute('hidden')).toBe(false);
		expect(rows().length).toBe(2);
		expect(section().textContent).toContain('npm run dev');
		expect(section().textContent).toContain('npm test');
	});

	it('shows the details of a job when its row opens: its state, its folder and its PID', () => {
		current().jobs.set('j1', job('j1', 'completed', 'npm test'));
		render();
		open();
		const details = rows()[0].querySelector<HTMLElement>('[data-collapsible-content]')!;
		expect(details.hasAttribute('hidden')).toBe(true);
		rows()[0].querySelector<HTMLElement>('button')!.click();
		flushSync();
		expect(details.hasAttribute('hidden')).toBe(false);
		expect(details.textContent).toContain('Terminée');
		expect(details.textContent).toContain('/work/demo');
		expect(details.textContent).toContain('4242');
	});
});

describe('stop, for a running job only', () => {
	it('offers « Arrêter » for the running job, and not for a finished one', () => {
		current().jobs.set('j1', job('j1', 'running', 'npm run dev'));
		current().jobs.set('j2', job('j2', 'completed', 'npm test'));
		render();
		open();
		const stops = [...section().querySelectorAll<HTMLButtonElement>('button[aria-label^="Arrêter "]')];
		expect(stops.map((b) => b.getAttribute('aria-label'))).toEqual(['Arrêter npm run dev']);
	});
});

describe('the refresh', () => {
	it('says « Actualisation… » while the engine has not answered an empty list', () => {
		render();
		expect(summary()).toBe('Actualisation…');
	});

	it('reads « Aucune tâche » once the engine answered with no job, and the section has nothing to open', () => {
		render();
		current().lastAgentControl = { action: 'jobs', ok: true, text: '', at: T0 };
		flushSync();
		expect(summary()).toBe('Aucune tâche');
		expect(trigger().hasAttribute('disabled')).toBe(true);
	});

	it('says why the engine refused the refresh, as an alert', () => {
		render();
		current().lastAgentControl = { action: 'jobs', ok: false, text: 'moteur occupé', at: T0 };
		flushSync();
		expect(target.querySelector('[role="alert"]')?.textContent).toContain('Actualisation impossible : moteur occupé');
	});
});
