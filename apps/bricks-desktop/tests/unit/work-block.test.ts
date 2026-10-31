// @vitest-environment happy-dom
// The work block (timeline/WorkBlock.svelte): the steps of a run under a thin bar. While the run
// works, the bar counts its time and shows the steps; folded, it names the current step. When the
// run ends it reads « A travaillé … », folds by itself, and what asks the person something stays
// visible while it is folded. Mounted in happy-dom with fake timers, and fed by the reducer.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import WorkBlock from '../../src/lib/features/conversation/timeline/WorkBlock.svelte';
import { timelineBlocks, type WorkBlock as WorkBlockData } from '../../src/lib/features/conversation/runBlocks';
import { Conversation } from '../../src/lib/state/conversation.svelte';
import type { ApprovalRequest } from '../../src/lib/ipc/engine';
import { EnvelopeFeed, PROJECT_ID, T0, openedSession } from './fixtures';

const RUN = 'run-bar';

const APPROVAL: ApprovalRequest = {
	approval_id: 'ap-1',
	tool_call_id: 'tc-9',
	tool: 'Bash',
	level: 'execute',
	description: 'Lancer les tests',
	input: { command: 'npm test' },
	preview: null,
};

/** A conversation whose run started at `T0` and is still going. */
function runningRun() {
	const conv = new Conversation(openedSession(), PROJECT_ID);
	const feed = new EnvelopeFeed({ runId: RUN });
	conv.apply(feed.next({ type: 'run_started', prompt: 'Travaille', attachments: [], model: 'm' }, { at: T0 }));
	return { conv, feed };
}

type Feed = ReturnType<typeof runningRun>['feed'];

function readCall(conv: Conversation, feed: Feed, id: string, name: string) {
	conv.apply(feed.next({ type: 'tool_started', tool_call_id: id, name, input: { file_path: 'a.ts' } }));
	conv.apply(feed.next({ type: 'tool_finished', tool_call_id: id, name, is_error: false, duration_ms: 12, output: 'ok' }));
}

/** The work block's steps and reasoning, as the timeline gives them to the block. */
function barOf(conv: Conversation): Pick<WorkBlockData, 'items' | 'answerReasoning'> {
	const block = timelineBlocks(conv.items, conv.items, conv.runs, conv.runningRunId).find(
		(b): b is WorkBlockData => b.kind === 'work' && b.runId === RUN,
	);
	return block ?? { items: [], answerReasoning: null };
}

let target: HTMLDivElement;
let app: ReturnType<typeof mount> | null = null;

/** Mounts the block. Its steps are read through getters, so the block follows the run as it goes. */
function show(conv: Conversation): void {
	app = mount(WorkBlock, {
		target,
		props: {
			conversation: conv,
			runId: RUN,
			get items() {
				return barOf(conv).items;
			},
			get answerReasoning() {
				return barOf(conv).answerReasoning;
			},
		},
	});
	flushSync();
}

/**
 * The text on screen. A folded block keeps its steps in the DOM, as closed collapsible content
 * (`data-state="closed"`): once folded they are not on screen, so they are left out. happy-dom has
 * no closing animation to wait for, so the closed content is what the settled state looks like.
 */
function text(): string {
	const shown = target.cloneNode(true) as HTMLElement;
	for (const closed of shown.querySelectorAll('[data-collapsible-content][data-state="closed"], [hidden]')) {
		closed.remove();
	}
	return (shown.textContent ?? '').replace(/\s+/g, ' ').trim();
}

/** The arrow of the bar: the first button of the block, when the block has steps to fold. */
function trigger(): HTMLButtonElement | null {
	return target.querySelector('button');
}

function expanded(): string | null {
	return trigger()?.getAttribute('aria-expanded') ?? null;
}

function toggle(): void {
	trigger()!.click();
	flushSync();
}

beforeEach(() => {
	vi.useFakeTimers();
	vi.setSystemTime(T0);
	target = document.createElement('div');
	document.body.appendChild(target);
});

afterEach(() => {
	if (app) void unmount(app);
	app = null;
	target.remove();
	vi.useRealTimers();
});

describe('WorkBlock while the run works', () => {
	it('is open at first: the bar counts the time and every step is shown', () => {
		const { conv, feed } = runningRun();
		readCall(conv, feed, 'tc-1', 'Read');
		conv.apply(feed.next({ type: 'tool_started', tool_call_id: 'tc-2', name: 'Grep', input: { pattern: 'x' } }));
		vi.setSystemTime(T0 + 27_000);
		show(conv);
		expect(text()).toContain('Travaille depuis 27s');
		expect(expanded()).toBe('true');
		// Open, the steps are in their own lines: the current step is not repeated on the bar.
		expect(text().split('Grep')).toHaveLength(2);
		expect(text()).toContain('Read');
	});

	it('folded by its arrow, the bar names the current step and hides the others', () => {
		const { conv, feed } = runningRun();
		readCall(conv, feed, 'tc-1', 'Read');
		conv.apply(feed.next({ type: 'tool_started', tool_call_id: 'tc-2', name: 'Grep', input: { pattern: 'x' } }));
		vi.setSystemTime(T0 + 27_000);
		show(conv);
		toggle();
		expect(expanded()).toBe('false');
		expect(text()).toContain('Travaille depuis 27s');
		expect(text()).toContain('Grep');
		expect(text()).not.toContain('Read');
	});

	it('an empty run has no arrow to fold: its bar is the time alone', () => {
		const { conv } = runningRun();
		vi.setSystemTime(T0 + 3_000);
		show(conv);
		expect(trigger()).toBeNull();
		expect(text()).toBe('Travaille depuis 3s');
	});

	it('a pending approval stays visible when the bar is folded, and goes once it is decided', () => {
		const { conv, feed } = runningRun();
		readCall(conv, feed, 'tc-1', 'Read');
		conv.apply(feed.next({ type: 'approval_requested', approval: APPROVAL }, { at: T0 + 1_000 }));
		show(conv);
		expect(expanded()).toBe('true');
		toggle();
		expect(expanded()).toBe('false');
		expect(text()).toContain('Lancer les tests');
		// The steps stay folded: the finished call's duration is not on screen.
		expect(text()).not.toContain('12ms');

		conv.apply(
			feed.next({ type: 'approval_resolved', approval_id: 'ap-1', tool_call_id: 'tc-9', decision: 'allow', by: 'user' }),
		);
		flushSync();
		expect(text()).not.toContain('Lancer les tests');
	});
});

describe('WorkBlock once the run has ended', () => {
	it('reads « A travaillé » with the time, and folds by itself', () => {
		const { conv, feed } = runningRun();
		readCall(conv, feed, 'tc-1', 'Read');
		show(conv);
		expect(expanded()).toBe('true');

		vi.setSystemTime(T0 + 6_000);
		conv.apply(feed.next({ type: 'run_finished', outcome: 'succeeded', text: '', turns: 1 }, { at: T0 + 6_000 }));
		flushSync();
		expect(text()).toBe('A travaillé 6s');
		expect(expanded()).toBe('false');
		// The clock of the bar has stopped: the time it reads is the run's, not the wall clock's.
		vi.advanceTimersByTime(60_000);
		flushSync();
		expect(text()).toBe('A travaillé 6s');
	});

	it.each([
		['failed', 'Échec'],
		['incomplete', 'Incomplet'],
		['cancelled', 'Annulé'],
	] as const)('a run that ended as %s says « %s » after its time', (outcome, label) => {
		const { conv, feed } = runningRun();
		readCall(conv, feed, 'tc-1', 'Bash');
		conv.apply(feed.next({ type: 'run_finished', outcome, text: '', turns: 1 }, { at: T0 + 6_000 }));
		show(conv);
		expect(text()).toBe(`A travaillé 6s · ${label}`);
	});
});
