// @vitest-environment happy-dom
// The card of a question of `Ask` (QuestionCard.svelte), mounted in happy-dom with the IPC
// boundary recorded: the header, the recommended choice preselected, « Passer » and « Envoyer »
// (⌘↩ too), « Autre » with its text, the refusal of the engine, the countdown and its colours
// (fake clock), and the one line that says how a closed question ended.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import QuestionCard from '../../src/lib/features/conversation/timeline/QuestionCard.svelte';
import ToolRow from '../../src/lib/features/conversation/timeline/ToolRow.svelte';
import { Conversation } from '../../src/lib/state/conversation.svelte';
import type { QuestionOutcome, QuestionRequest } from '../../src/lib/ipc/engine';
import { PROJECT_ID, SESSION_ID, T0, openedSession } from './fixtures';

const ipc = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => {
	class Channel {
		onmessage: ((event: unknown) => void) | null = null;
	}
	return { Channel, invoke: (cmd: string, args?: unknown) => ipc.invoke(cmd, args) };
});
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }));

const QUESTION: QuestionRequest = {
	question_id: 'q-1',
	tool_call_id: 'tc-1',
	question: 'Quelle base de données ?',
	options: [{ label: 'Postgres', description: 'serveur partagé' }, { label: 'SQLite' }],
	recommended: 1
};

/** The same question with a timer of one minute, ending at `T0 + 60 s`. */
const TIMED: QuestionRequest = { ...QUESTION, timeout_seconds: 60, deadline_at: T0 + 60_000 };

/** The engine's answer to each command: a refusal when `refusal` is set. */
let refusal: { code: string; message: string } | null = null;

let target: HTMLDivElement;
let component: ReturnType<typeof mount> | null = null;
let conversation: Conversation;

beforeEach(() => {
	vi.useFakeTimers({ toFake: ['Date', 'setInterval', 'clearInterval'] });
	vi.setSystemTime(T0);
	refusal = null;
	ipc.invoke.mockReset();
	ipc.invoke.mockImplementation(async (cmd: string) => {
		if (cmd === 'session_command') {
			if (refusal) throw refusal;
			return undefined;
		}
		throw new Error(`commande IPC non prévue dans ce test : ${cmd}`);
	});
	target = document.createElement('div');
	document.body.appendChild(target);
	conversation = new Conversation(openedSession(), PROJECT_ID);
});

afterEach(() => {
	if (component) void unmount(component);
	component = null;
	target.remove();
	vi.useRealTimers();
});

function show(question: QuestionRequest) {
	component = mount(QuestionCard, { target, props: { question, conversation } });
	flushSync();
}

/** Lets the pending answers of the card settle, then renders what they changed. */
async function settle() {
	for (let i = 0; i < 10; i++) await Promise.resolve();
	flushSync();
}

/** The answers the engine received, in order. */
function answers(): unknown[] {
	return ipc.invoke.mock.calls
		.filter(([cmd]) => cmd === 'session_command')
		.map(([, args]) => (args as { command: { answer?: unknown } }).command.answer);
}

function button(label: string): HTMLButtonElement {
	const found = [...target.querySelectorAll('button')].find((b) => b.textContent?.trim().startsWith(label));
	if (!found) throw new Error(`no button « ${label} »`);
	return found;
}

function radios(): HTMLInputElement[] {
	return [...target.querySelectorAll<HTMLInputElement>('input[type="radio"]')];
}

function choose(index: number) {
	const radio = radios()[index];
	radio.checked = true;
	radio.dispatchEvent(new Event('change', { bubbles: true }));
	flushSync();
}

function countdown(): HTMLElement | null {
	return target.querySelector<HTMLElement>('[aria-label^="Temps restant"]');
}

describe('a waiting question', () => {
	it('says it needs the person, and shows the question as its heading', () => {
		show(QUESTION);
		expect(target.textContent).toContain('Requiert votre intervention');
		expect(target.querySelector('h2')?.textContent?.trim()).toBe('Quelle base de données ?');
		expect(target.querySelector('section')?.getAttribute('aria-labelledby')).toBeTruthy();
	});

	it('offers each option with its description, then « Autre »', () => {
		show(QUESTION);
		expect(radios()).toHaveLength(3);
		expect(target.textContent).toContain('serveur partagé');
		expect(target.textContent).toContain('Autre');
	});

	it('preselects the recommended option and marks it « (Recommandé) »', () => {
		show(QUESTION);
		expect(radios().map((r) => r.checked)).toEqual([false, true, false]);
		expect(target.textContent).toMatch(/SQLite\s*\(Recommandé\)/);
		expect(target.textContent).not.toMatch(/Postgres\s*\(Recommandé\)/);
	});

	it('preselects nothing when the model recommends no option', () => {
		show({ ...QUESTION, recommended: undefined });
		expect(radios().map((r) => r.checked)).toEqual([false, false, false]);
		expect(target.textContent).not.toContain('(Recommandé)');
	});

	it('« Envoyer » sends the preselected choice without another click', async () => {
		show(QUESTION);
		button('Envoyer').click();
		await settle();
		expect(answers()).toEqual([{ kind: 'choice', choice: 1 }]);
	});

	it('« Envoyer » sends the option the person picked', async () => {
		show(QUESTION);
		choose(0);
		button('Envoyer').click();
		await settle();
		expect(answers()).toEqual([{ kind: 'choice', choice: 0 }]);
	});

	it('« Passer » sends a skip', async () => {
		show(QUESTION);
		button('Passer').click();
		await settle();
		expect(answers()).toEqual([{ kind: 'skip' }]);
	});

	it('⌘↩ sends the choice, and a plain Enter does nothing', async () => {
		show(QUESTION);
		const section = target.querySelector('section') as HTMLElement;
		section.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
		await settle();
		expect(answers()).toEqual([]);

		section.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', metaKey: true, bubbles: true }));
		await settle();
		expect(answers()).toEqual([{ kind: 'choice', choice: 1 }]);
	});

	it('« Autre » asks for a text, and « Envoyer » waits for a non-blank one', async () => {
		show(QUESTION);
		choose(2);
		const input = target.querySelector<HTMLInputElement>('input[aria-label="Votre réponse"]');
		expect(input).not.toBeNull();
		expect(button('Envoyer').disabled).toBe(true);

		input!.value = '   ';
		input!.dispatchEvent(new Event('input', { bubbles: true }));
		flushSync();
		expect(button('Envoyer').disabled).toBe(true);

		input!.value = '  Ni l’un ni l’autre  ';
		input!.dispatchEvent(new Event('input', { bubbles: true }));
		flushSync();
		expect(button('Envoyer').disabled).toBe(false);
		button('Envoyer').click();
		await settle();
		expect(answers()).toEqual([{ kind: 'other', text: 'Ni l’un ni l’autre' }]);
	});

	it('shows the engine’s refusal, and offers the buttons again', async () => {
		refusal = { code: 'invalid', message: 'Cette question n’est plus en attente.' };
		show(QUESTION);
		button('Passer').click();
		await settle();
		const alert = target.querySelector('[role="alert"]');
		expect(alert?.textContent).toContain('Cette question n’est plus en attente.');
		expect(button('Passer').disabled).toBe(false);
	});
});

describe('the countdown of a timed question', () => {
	it('shows the time left from the engine’s deadline, in minutes and seconds', () => {
		show(TIMED);
		expect(countdown()?.textContent?.trim()).toBe('1:00');
	});

	it('shows no countdown without a timer', () => {
		show(QUESTION);
		expect(countdown()).toBeNull();
	});

	it('is muted while 30 s or more are left, warning under 30 s, destructive under 10 s', () => {
		show(TIMED);
		expect(countdown()!.className).toContain('text-muted-foreground');

		vi.advanceTimersByTime(30_000);
		flushSync();
		expect(countdown()!.textContent?.trim()).toBe('0:30');
		expect(countdown()!.className).toContain('text-muted-foreground');

		vi.advanceTimersByTime(1_000);
		flushSync();
		expect(countdown()!.textContent?.trim()).toBe('0:29');
		expect(countdown()!.className).toContain('text-warning');
		expect(countdown()!.className).not.toContain('text-muted-foreground');

		vi.advanceTimersByTime(20_000);
		flushSync();
		expect(countdown()!.textContent?.trim()).toBe('0:09');
		expect(countdown()!.className).toContain('text-destructive');
		expect(countdown()!.className).not.toContain('text-warning');
	});

	it('stops at 0:00 once the deadline has passed', () => {
		show(TIMED);
		vi.advanceTimersByTime(75_000);
		flushSync();
		expect(countdown()!.textContent?.trim()).toBe('0:00');
	});
});

describe('the Ask line once its question is closed', () => {
	/** An `Ask` call that ended, with its question closed by `outcome`. */
	function showLine(outcome: QuestionOutcome) {
		conversation.items.push(
			{ kind: 'tool', id: 't-1', toolCallId: 'tc-1', name: 'Ask', input: {}, status: 'ok', durationMs: 1200 },
			{ kind: 'question', id: 'qi-1', question: QUESTION, outcome },
		);
		component = mount(ToolRow, { target, props: { item: conversation.items[0] as never, conversation } });
		flushSync();
	}

	it('counts the answered questions over the total, and shows no card', () => {
		showLine({ state: 'chosen', choice: 0 });
		expect(target.querySelector('section')).toBeNull();
		expect(target.textContent).toContain('1/1');
		expect(target.textContent).not.toContain('Postgres');
	});

	it('counts a skip as unanswered', () => {
		showLine({ state: 'skipped', reason: 'timeout' });
		expect(target.textContent).toContain('0/1');
		expect(target.querySelector('.text-destructive')).toBeNull();
	});

	it('marks the call failed with a red cross when the run was cancelled', () => {
		showLine({ state: 'skipped', reason: 'cancelled' });
		expect(target.querySelector('.text-destructive')).not.toBeNull();
		expect(target.textContent).toContain('Échec');
	});
});
