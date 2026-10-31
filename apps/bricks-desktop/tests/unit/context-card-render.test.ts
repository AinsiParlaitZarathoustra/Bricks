// @vitest-environment happy-dom
// The context card (ContextCard.svelte) mounted over a real Conversation: the share and the figures
// come from the status; with a breakdown the bar is split by source and a list names each source
// with its tokens and its share; without one the card keeps a plain bar and no list; a stale
// measure reads dashes.
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import ContextCard from '../../src/lib/features/context/ContextCard.svelte';
import type { ContextChars, ContextStatus } from '../../src/lib/ipc/engine';
import { Conversation } from '../../src/lib/state/conversation.svelte';
import { PROJECT_ID, openedSession, snapshot } from './fixtures';

const NONE: ContextChars = { instructions: 0, conversation: 0, tool_results: 0, tool_definitions: 0, other: 0 };

/** Occupation 349 600 of a 1 000 000 input budget; the text counted by source when given. */
function statusOf(chars?: Partial<ContextChars>): ContextStatus {
	const base = snapshot().context;
	return {
		...base,
		context_used: { ...base.context_used, tokens: 349_600 },
		input_limit: 1_000_000,
		...(chars ? { breakdown: { turn: 2, chars: { ...NONE, ...chars } } } : {})
	};
}

function conversationWith(context: ContextStatus, stale = false): Conversation {
	const conversation = new Conversation(openedSession({ snapshot: snapshot({ context }) }), PROJECT_ID);
	conversation.contextStale = stale;
	return conversation;
}

let target: HTMLDivElement;
let component: ReturnType<typeof mount> | null = null;

beforeEach(() => {
	target = document.createElement('div');
	document.body.appendChild(target);
});

afterEach(() => {
	if (component) void unmount(component);
	component = null;
	target.remove();
});

function render(conversation: Conversation) {
	component = mount(ContextCard, { target, props: { conversation } });
	flushSync();
}

const meter = () => target.querySelector<HTMLElement>('[role="meter"]')!;
const list = () => target.querySelector<HTMLElement>('[aria-label="Répartition du contexte"]');

describe('the share and the figures', () => {
	it('shows the share of the occupation and the figures in short', () => {
		render(conversationWith(statusOf()));
		expect(target.textContent).toContain('35 % utilisé');
		expect(target.textContent).toContain('349.6k / 1M');
	});

	it('the meter carries the share as its value and the figures as its text', () => {
		render(conversationWith(statusOf()));
		expect(meter().getAttribute('aria-valuenow')).toBe('35');
		expect(meter().getAttribute('aria-valuetext')).toBe('35 % (349.6k / 1M)');
	});

	it('a stale measure reads dashes, and the meter is empty', () => {
		render(conversationWith(statusOf({ conversation: 600, tool_results: 400 }), true));
		expect(target.textContent).toContain('— utilisé');
		expect(target.textContent).toContain('— / —');
		expect(meter().children.length).toBe(0);
		expect(list()).toBeNull();
	});
});

describe('without a breakdown: one plain bar, no list', () => {
	it('draws one bar at the share and shows no source list', () => {
		render(conversationWith(statusOf()));
		const bar = meter().firstElementChild as HTMLElement;
		expect(bar).not.toBeNull();
		expect(parseFloat(bar.style.width)).toBeCloseTo(34.96, 1);
		expect(bar.children.length).toBe(0);
		expect(list()).toBeNull();
	});
});

describe('with a breakdown: the bar is split and the list names each source', () => {
	it('lists each source that counted text, with its tokens and its share', () => {
		render(conversationWith(statusOf({ conversation: 600, tool_results: 400 })));
		const items = [...target.querySelectorAll<HTMLElement>('[aria-label="Répartition du contexte"] li')];
		expect(items.map((li) => li.textContent?.replace(/\s+/g, ' ').trim())).toEqual([
			'Messages 209.8k tokens 60,0 %',
			'Sorties d’outils 139.8k tokens 40,0 %',
			'Espace libre 650.4k tokens'
		]);
	});

	it('splits the bar into one segment per listed source', () => {
		render(conversationWith(statusOf({ conversation: 600, tool_results: 400 })));
		const bar = meter().firstElementChild as HTMLElement;
		expect(bar.children.length).toBe(2);
		expect(parseFloat(bar.style.width)).toBeCloseTo(34.96, 1);
	});

	it('lists only the sources that counted text', () => {
		render(conversationWith(statusOf({ conversation: 1_000 })));
		const items = target.querySelectorAll('[aria-label="Répartition du contexte"] li');
		expect(items.length).toBe(2);
		expect(items[0].textContent).toContain('Messages');
		expect(items[1].textContent).toContain('Espace libre');
	});

	it('ends with the room left in the input budget, without a share', () => {
		render(conversationWith(statusOf({ conversation: 600, tool_results: 400 })));
		const items = [...target.querySelectorAll<HTMLElement>('[aria-label="Répartition du contexte"] li')];
		const last = items.at(-1)?.textContent?.replace(/\s+/g, ' ').trim();
		expect(last).toBe('Espace libre 650.4k tokens');
		expect(last).not.toContain('%');
	});

	it('shows no approximation sign and no tilde', () => {
		render(conversationWith(statusOf({ conversation: 600, tool_results: 400 })));
		expect(target.textContent).not.toContain('~');
		expect(target.textContent).not.toContain('≈');
	});
});
