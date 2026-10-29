// The question timer on the wire: every message (submit, revise) carries the timer of the
// preferences as the engine takes it (off, smart or custom with its seconds); an answer to a
// question is not a message and carries none. The IPC boundary is `invoke` of
// `@tauri-apps/api/core`, the real client runs over it. A fresh app store per test.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Command, QuestionAnswer } from '../../src/lib/ipc/engine';
import { PROJECT_ID, SESSION_ID } from './fixtures';

type AppStore = (typeof import('../../src/lib/state/app.svelte'))['app'];

const ipc = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => {
	class Channel {
		onmessage: ((event: unknown) => void) | null = null;
	}
	return { Channel, invoke: (cmd: string, args?: unknown) => ipc.invoke(cmd, args) };
});
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }));

let app: AppStore;
/** The refusal the engine answers the next session_command with, if any. */
let refusal: { code: string; message: string } | null = null;

beforeEach(async () => {
	vi.useFakeTimers({ toFake: ['Date', 'setTimeout', 'clearTimeout'] });
	vi.resetModules();
	({ app } = await import('../../src/lib/state/app.svelte'));
	refusal = null;
	ipc.invoke.mockReset();
	ipc.invoke.mockImplementation(async (cmd: string) => {
		switch (cmd) {
			case 'session_command':
				if (refusal) throw refusal;
				return undefined;
			case 'prefs_set':
				return undefined;
			default:
				throw new Error(`commande IPC non prévue dans ce test : ${cmd}`);
		}
	});
	app.currentProjectId = PROJECT_ID;
});

afterEach(() => {
	vi.useRealTimers();
});

/** The commands the engine received, in order. */
function commands(): Command[] {
	return ipc.invoke.mock.calls
		.filter(([cmd]) => cmd === 'session_command')
		.map(([, args]) => (args as { command: Command }).command);
}

function setTimer(ask_timer: { enabled: boolean; mode: 'smart' | 'custom'; seconds: number }) {
	app.setPrefs({ ask_timer });
}

const TEXT = { kind: 'text' as const, text: 'Bonjour' };

describe('askTimer(): the preferences as the engine takes them', () => {
	it('is off while the timer is disabled, whatever its mode and seconds', () => {
		setTimer({ enabled: false, mode: 'custom', seconds: 120 });
		expect(app.askTimer()).toEqual({ mode: 'off' });
		setTimer({ enabled: false, mode: 'smart', seconds: 300 });
		expect(app.askTimer()).toEqual({ mode: 'off' });
	});

	it('is smart when enabled in smart mode: the model sets each duration', () => {
		setTimer({ enabled: true, mode: 'smart', seconds: 300 });
		expect(app.askTimer()).toEqual({ mode: 'smart' });
	});

	it('is custom with the stored seconds when enabled in custom mode', () => {
		setTimer({ enabled: true, mode: 'custom', seconds: 120 });
		expect(app.askTimer()).toEqual({ mode: 'custom', seconds: 120 });
	});

	it('starts off: the default preferences have the timer disabled', () => {
		expect(app.prefs.ask_timer).toEqual({ enabled: false, mode: 'smart', seconds: 300 });
		expect(app.askTimer()).toEqual({ mode: 'off' });
	});
});

describe('app.command: every message carries the timer', () => {
	it('a submit sends the timer as set now: off when disabled', async () => {
		setTimer({ enabled: false, mode: 'smart', seconds: 300 });
		expect(await app.command(SESSION_ID, { type: 'submit', prompt: { blocks: [TEXT] } })).toBeNull();
		expect(commands()).toEqual([{ type: 'submit', prompt: { blocks: [TEXT], ask_timer: { mode: 'off' } } }]);
	});

	it('a submit sends smart', async () => {
		setTimer({ enabled: true, mode: 'smart', seconds: 300 });
		await app.command(SESSION_ID, { type: 'submit', prompt: { blocks: [TEXT] } });
		expect(commands()[0]).toEqual({ type: 'submit', prompt: { blocks: [TEXT], ask_timer: { mode: 'smart' } } });
	});

	it('a submit sends custom with the seconds', async () => {
		setTimer({ enabled: true, mode: 'custom', seconds: 120 });
		await app.command(SESSION_ID, { type: 'submit', prompt: { blocks: [TEXT], skills: ['pdf'] } });
		expect(commands()[0]).toEqual({
			type: 'submit',
			prompt: { blocks: [TEXT], skills: ['pdf'], ask_timer: { mode: 'custom', seconds: 120 } }
		});
	});

	it('a revision carries the timer too', async () => {
		setTimer({ enabled: true, mode: 'custom', seconds: 90 });
		await app.command(SESSION_ID, {
			type: 'revise',
			user_index: 0,
			original: 'Bonjour',
			prompt: { blocks: [TEXT] }
		});
		expect(commands()[0]).toEqual({
			type: 'revise',
			user_index: 0,
			original: 'Bonjour',
			prompt: { blocks: [TEXT], ask_timer: { mode: 'custom', seconds: 90 } }
		});
	});

	it('a later change of the setting applies to the next message only', async () => {
		setTimer({ enabled: true, mode: 'custom', seconds: 120 });
		await app.command(SESSION_ID, { type: 'submit', prompt: { blocks: [TEXT] } });
		setTimer({ enabled: false, mode: 'custom', seconds: 120 });
		await app.command(SESSION_ID, { type: 'submit', prompt: { blocks: [TEXT] } });

		const [first, second] = commands() as Array<Extract<Command, { type: 'submit' }>>;
		expect(first.prompt.ask_timer).toEqual({ mode: 'custom', seconds: 120 });
		expect(second.prompt.ask_timer).toEqual({ mode: 'off' });
	});

	it('an answer to a question is not a message: it carries no timer', async () => {
		setTimer({ enabled: true, mode: 'custom', seconds: 120 });
		const conv = { sessionId: SESSION_ID } as unknown as Parameters<AppStore['answerQuestion']>[0];
		const answer: QuestionAnswer = { kind: 'choice', choice: 1 };

		expect(await app.answerQuestion(conv, 'q-1', answer)).toBeNull();

		expect(commands()).toEqual([{ type: 'answer_question', question_id: 'q-1', answer: { kind: 'choice', choice: 1 } }]);
		expect(commands()[0]).not.toHaveProperty('prompt');
		expect(commands()[0]).not.toHaveProperty('ask_timer');
	});

	it('an answer sends its free text and its skip as the engine reads them', async () => {
		const conv = { sessionId: SESSION_ID } as unknown as Parameters<AppStore['answerQuestion']>[0];
		await app.answerQuestion(conv, 'q-2', { kind: 'other', text: 'Ni l’un ni l’autre' });
		await app.answerQuestion(conv, 'q-3', { kind: 'skip' });
		expect(commands()).toEqual([
			{ type: 'answer_question', question_id: 'q-2', answer: { kind: 'other', text: 'Ni l’un ni l’autre' } },
			{ type: 'answer_question', question_id: 'q-3', answer: { kind: 'skip' } }
		]);
	});

	it('a refused command returns the refusal message and does not throw', async () => {
		refusal = { code: 'invalid', message: 'la question n’est plus en attente' };
		const conv = { sessionId: SESSION_ID } as unknown as Parameters<AppStore['answerQuestion']>[0];
		const error = await app.answerQuestion(conv, 'q-1', { kind: 'skip' });
		expect(error).toBe('la question n’est plus en attente');
	});
});
