// @vitest-environment happy-dom
// The « Travaille depuis … » line (WorkingLine.svelte), mounted while a run is going: it counts
// from the run's start (engine timestamp), in whole seconds, and only while it is mounted.
// Without a known start it reads « Travaille… ». Mounted in happy-dom with fake timers.
import { readFileSync } from 'node:fs';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import WorkingLine from '../../src/lib/features/conversation/timeline/WorkingLine.svelte';
import { Conversation } from '../../src/lib/state/conversation.svelte';
import { EnvelopeFeed, PROJECT_ID, T0, openedSession, snapshot } from './fixtures';

const RUN = 'run-work';

/** A conversation whose run started at `T0`, with the run still going. */
function runningConversation(): Conversation {
	const conv = new Conversation(openedSession(), PROJECT_ID);
	const feed = new EnvelopeFeed({ runId: RUN });
	conv.apply(feed.next({ type: 'run_started', prompt: 'Travaille', attachments: [], model: 'm' }, { runId: RUN, at: T0 }));
	return conv;
}

/** A conversation the snapshot says is running, with no start known for its run. */
function runningWithoutStart(): Conversation {
	return new Conversation(
		openedSession({ snapshot: snapshot({ activity: { state: 'running', run_id: 'run-unknown' } }) }),
		PROJECT_ID,
	);
}

let target: HTMLDivElement;
let app: ReturnType<typeof mount> | null = null;

function show(conversation: Conversation): void {
	app = mount(WorkingLine, { target, props: { conversation } });
	flushSync();
}

function text(): string {
	return (target.textContent ?? '').replace(/\s+/g, ' ').trim();
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

describe('WorkingLine: the elapsed time of the run', () => {
	it('shows « Travaille depuis » and the seconds since the run started', () => {
		const conv = runningConversation();
		vi.setSystemTime(T0 + 27_000);
		show(conv);
		expect(text()).toBe('Travaille depuis 27s');
	});

	it('shows minutes and seconds past one minute, as « 1m 29s »', () => {
		const conv = runningConversation();
		vi.setSystemTime(T0 + 89_000);
		show(conv);
		expect(text()).toBe('Travaille depuis 1m 29s');
	});

	it('never shows milliseconds: a part of a second is not yet counted', () => {
		const conv = runningConversation();
		vi.setSystemTime(T0 + 1_999);
		show(conv);
		expect(text()).toBe('Travaille depuis 1s');
	});

	// Known defect: under one whole second the line reads « 0ms », because the whole-second value
	// (0 ms) goes through formatDuration, which spells anything under 1000 ms in milliseconds.
	// Turn into `it` once WorkingLine shows « 0s ».
	it('under one second the line reads « 0s », not « 0ms »', () => {
		const conv = runningConversation();
		vi.setSystemTime(T0 + 999);
		show(conv);
		expect(text()).toBe('Travaille depuis 0s');
	});

	it('counts again every second while mounted', () => {
		const conv = runningConversation();
		vi.setSystemTime(T0 + 1_000);
		show(conv);
		expect(text()).toBe('Travaille depuis 1s');
		vi.advanceTimersByTime(1_000);
		flushSync();
		expect(text()).toBe('Travaille depuis 2s');
		vi.advanceTimersByTime(25_000);
		flushSync();
		expect(text()).toBe('Travaille depuis 27s');
	});

	it('stops its timer when it is unmounted', () => {
		const conv = runningConversation();
		show(conv);
		expect(vi.getTimerCount()).toBeGreaterThan(0);
		void unmount(app!);
		app = null;
		expect(vi.getTimerCount()).toBe(0);
	});
});

describe('WorkingLine: without a known start', () => {
	it('reads « Travaille… » and no duration', () => {
		show(runningWithoutStart());
		expect(text()).toBe('Travaille…');
		expect(text()).not.toMatch(/depuis/);
	});

	it('reads « Travaille… » when the running run has no start yet', () => {
		const conv = new Conversation(openedSession(), PROJECT_ID);
		conv.runningRunId = 'run-no-start';
		show(conv);
		expect(text()).toBe('Travaille…');
	});
});

describe('WorkingLine: the old waiting line is gone', () => {
	it('never renders « En cours… »', () => {
		show(runningConversation());
		expect(text()).not.toContain('En cours');
		// The npm scripts run from apps/bricks-desktop.
		const source = 'src/lib/features/conversation/timeline/WorkingLine.svelte';
		expect(readFileSync(source, 'utf8')).not.toContain('En cours');
	});
});
