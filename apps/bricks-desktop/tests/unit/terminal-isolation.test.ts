// Isolation of the terminals of the bottom pane: one visible terminal at a time, and no
// output, input, resize or kill ever reaches another terminal. xterm, the PTY commands
// and the DOM are fakes (see the fakes below); the session and store rules are the real
// ones from src/lib/features/terminal. The pane component itself is not mounted: `Pane`
// mirrors its effect (cleanup first, then attach and start the active terminal).
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ITheme, ITerminalOptions } from '@xterm/xterm';
import type { PtyEvent } from '../../src/lib/ipc/bindings/PtyEvent';
import type { PtyInfo } from '../../src/lib/ipc/bindings/PtyInfo';
import type { PtyOpenRequest } from '../../src/lib/ipc/bindings/PtyOpenRequest';
import {
	TerminalSession,
	type PtyPort,
	type TerminalDeps,
	type XtermPort
} from '../../src/lib/features/terminal/terminal-session.svelte';
import { TerminalStore } from '../../src/lib/features/terminal/terminal-store.svelte';

// ─── Fakes ───────────────────────────────────────────────────────────────────

/** Just enough of an element for the session: children, parent, size. */
class FakeEl {
	children: FakeEl[] = [];
	parentElement: FakeEl | null = null;
	clientWidth = 800;
	clientHeight = 300;
	appendChild(child: FakeEl): FakeEl {
		child.remove();
		child.parentElement = this;
		this.children.push(child);
		return child;
	}
	remove(): void {
		if (this.parentElement) {
			this.parentElement.children = this.parentElement.children.filter((c) => c !== this);
			this.parentElement = null;
		}
	}
}

const CELL_W = 8;
const CELL_H = 16;
const THEME: ITheme = { background: '#101010' };
const FONT = 'Google Sans Code, monospace';

type LookOptions = Pick<ITerminalOptions, 'fontFamily' | 'theme'>;

class FakeXterm implements XtermPort {
	cols = 80;
	rows = 24;
	/** Every value assigned to fontFamily, in order (xterm measures glyphs after each change). */
	readonly fontWrites: string[] = [];
	readonly options: LookOptions;
	opened: FakeEl | null = null;
	openCount = 0;
	/** Decoded text written to this terminal. */
	readonly output: string[] = [];
	fitCalls = 0;
	disposed = false;
	readonly #dataListeners: Array<(data: string) => void> = [];
	readonly #resizeListeners: Array<(size: { cols: number; rows: number }) => void> = [];

	constructor(look: LookOptions) {
		const fontWrites = this.fontWrites;
		this.options = new Proxy({} as LookOptions, {
			set(target, key, value) {
				(target as Record<string, unknown>)[key as string] = value;
				if (key === 'fontFamily') fontWrites.push(value as string);
				return true;
			}
		});
		// Like the real constructor: the look is given once, at creation.
		this.options.fontFamily = look.fontFamily;
		this.options.theme = look.theme;
	}

	open(parent: HTMLElement): void {
		this.openCount += 1;
		this.opened = parent as unknown as FakeEl;
	}
	write(data: string | Uint8Array): void {
		if (this.disposed) throw new Error('write after dispose');
		this.output.push(typeof data === 'string' ? data : new TextDecoder().decode(data));
	}
	focus(): void {}
	dispose(): void {
		this.disposed = true;
	}
	onData(listener: (data: string) => void) {
		this.#dataListeners.push(listener);
		return { dispose: () => {} };
	}
	onResize(listener: (size: { cols: number; rows: number }) => void) {
		this.#resizeListeners.push(listener);
		return { dispose: () => {} };
	}

	/** A keystroke typed into this terminal's own input. */
	type(data: string): void {
		for (const listener of this.#dataListeners) listener(data);
	}

	/** What FitAddon does: sizes the grid from the host's parent, never from a hidden one. */
	fit(): void {
		this.fitCalls += 1;
		const container = this.opened?.parentElement;
		if (!container || container.clientWidth === 0 || container.clientHeight === 0) return;
		this.#resize(Math.floor(container.clientWidth / CELL_W), Math.floor(container.clientHeight / CELL_H));
	}

	#resize(cols: number, rows: number): void {
		if (cols === this.cols && rows === this.rows) return;
		this.cols = cols;
		this.rows = rows;
		for (const listener of this.#resizeListeners) listener({ cols, rows });
	}
}

interface OpenedPty {
	id: string;
	request: PtyOpenRequest;
	onEvent: (event: PtyEvent) => void;
}

class FakePty implements PtyPort {
	readonly opens: OpenedPty[] = [];
	readonly writes: Array<{ id: string; data: string }> = [];
	readonly resizes: Array<{ id: string; cols: number; rows: number }> = [];
	readonly kills: string[] = [];
	#seq = 0;

	open(request: PtyOpenRequest, onEvent: (event: PtyEvent) => void): Promise<PtyInfo> {
		this.#seq += 1;
		const id = `pty-${this.#seq}`;
		this.opens.push({ id, request, onEvent });
		return Promise.resolve({
			id,
			project_id: request.project_id,
			session_id: request.session_id,
			shell: 'zsh',
			cwd: `/projects/${request.project_id}`
		});
	}
	write(id: string, data: string): Promise<void> {
		this.writes.push({ id, data });
		return Promise.resolve();
	}
	resize(id: string, cols: number, rows: number): Promise<void> {
		this.resizes.push({ id, cols, rows });
		return Promise.resolve();
	}
	kill(id: string): Promise<void> {
		this.kills.push(id);
		return Promise.resolve();
	}

	/** The shell prints `text` (base64 encoded, as the engine sends it). */
	print(id: string, text: string): void {
		this.#emit(id, { kind: 'data', data: Buffer.from(text, 'utf8').toString('base64') });
	}
	exit(id: string, code: number | null): void {
		this.#emit(id, { kind: 'exit', code });
	}
	#emit(id: string, event: PtyEvent): void {
		const opened = this.opens.find((o) => o.id === id);
		if (!opened) throw new Error(`no pty ${id}`);
		opened.onEvent(event);
	}
}

interface World {
	pty: FakePty;
	/** Every xterm created, in creation order (one per session). */
	engines: FakeXterm[];
	errors: unknown[];
	font: { value: string };
	deps: TerminalDeps;
}

function makeWorld(): World {
	const pty = new FakePty();
	const engines: FakeXterm[] = [];
	const errors: unknown[] = [];
	const font = { value: FONT };
	const deps: TerminalDeps = {
		createEngine(look) {
			const term = new FakeXterm({ fontFamily: look.font, theme: look.theme });
			engines.push(term);
			return { term, fit: () => term.fit() };
		},
		createHost: () => new FakeEl() as unknown as HTMLElement,
		pty,
		font: () => font.value,
		theme: () => THEME,
		errorMessage: (e) => String(e),
		onError: (e) => errors.push(e)
	};
	return { pty, engines, errors, font, deps };
}

/** Lets promise callbacks run and any debounce timer due within `ms` fire. */
async function settle(ms = 0): Promise<void> {
	await vi.advanceTimersByTimeAsync(ms);
	for (let i = 0; i < 5; i++) await Promise.resolve();
}

/** A terminal with the xterm instance the store gave it. */
interface Tab {
	session: TerminalSession;
	term: FakeXterm;
}

function createTab(world: World, store: TerminalStore, projectId: string, sessionId: string | null = null): Tab {
	const index = world.engines.length;
	const session = store.create(projectId, sessionId);
	return { session, term: world.engines[index] };
}

/** Starts a tab's shell and returns the PTY id that its open call created. */
async function startTab(world: World, tab: Tab): Promise<string> {
	const before = world.pty.opens.length;
	tab.session.start();
	await settle();
	const opened = world.pty.opens[before];
	if (!opened) throw new Error('start did not open a pty');
	return opened.id;
}

/**
 * Mirrors the effect of TerminalPanel: cleanup of the previous terminal (detach),
 * then attach and start the active terminal of the project in one container.
 */
class Pane {
	readonly container = new FakeEl();
	#shown: TerminalSession | null = null;
	readonly #store: TerminalStore;

	constructor(store: TerminalStore) {
		this.#store = store;
	}

	get shown(): TerminalSession | null {
		return this.#shown;
	}

	showActive(projectId: string | null): void {
		if (this.#shown) this.#shown.detach();
		this.#shown = null;
		if (!projectId) return;
		const id = this.#store.activeId(projectId);
		const session = this.#store.list(projectId).find((s) => s.id === id) ?? null;
		if (!session) return;
		session.attach(this.container as unknown as HTMLElement);
		if (session.status === 'idle') session.start();
		this.#shown = session;
	}

	/** The xterm hosts currently in the container. */
	hosts(): FakeEl[] {
		return this.container.children;
	}
}

/** The container holds exactly these hosts (by identity), in order. */
function expectHosts(pane: Pane, hosts: HTMLElement[]): void {
	const actual = pane.hosts() as unknown as HTMLElement[];
	expect(actual.length).toBe(hosts.length);
	hosts.forEach((host, i) => expect(actual[i]).toBe(host));
}

function decoded(term: FakeXterm): string {
	return term.output.join('');
}

// ─── Tests ───────────────────────────────────────────────────────────────────

describe('terminal isolation', () => {
	let world: World;
	let store: TerminalStore;
	let pane: Pane;

	beforeEach(() => {
		vi.useFakeTimers();
		world = makeWorld();
		store = new TerminalStore(world.deps);
		pane = new Pane(store);
	});

	afterEach(() => {
		vi.useRealTimers();
	});

	describe('one visible terminal at a time', () => {
		it('a tab switch takes the previous terminal out of the container', async () => {
			const a = createTab(world, store, 'p1');
			const b = createTab(world, store, 'p1');
			await startTab(world, a);
			await startTab(world, b);
			store.activate('p1', a.session.id);
			pane.showActive('p1');
			expectHosts(pane, [a.session.host]);

			store.activate('p1', b.session.id);
			pane.showActive('p1');

			expectHosts(pane, [b.session.host]);
			expect(a.session.host.parentElement).toBeNull();
		});

		it('the previous terminal is detached, not disposed: its shell and xterm live on', async () => {
			const a = createTab(world, store, 'p1');
			const b = createTab(world, store, 'p1');
			const ptyA = await startTab(world, a);
			await startTab(world, b);
			store.activate('p1', a.session.id);
			pane.showActive('p1');

			store.activate('p1', b.session.id);
			pane.showActive('p1');

			expect(a.term.disposed).toBe(false);
			expect(world.pty.kills).not.toContain(ptyA);
			expect(a.term.openCount).toBe(1);
		});

		it('a rapid switch sequence never stacks hosts', async () => {
			const tabs = [createTab(world, store, 'p1'), createTab(world, store, 'p1'), createTab(world, store, 'p1')];
			for (const tab of tabs) await startTab(world, tab);
			pane.showActive('p1');
			for (const index of [1, 2, 0, 1, 1, 2, 0, 0, 2, 1]) {
				store.activate('p1', tabs[index].session.id);
				pane.showActive('p1');
				expect(pane.hosts().length).toBe(1);
			}
			expectHosts(pane, [tabs[1].session.host]);
		});

		it('attaching the same terminal twice to a container keeps one host', async () => {
			const a = createTab(world, store, 'p1');
			await startTab(world, a);
			a.session.attach(pane.container as unknown as HTMLElement);
			a.session.attach(pane.container as unknown as HTMLElement);
			expectHosts(pane, [a.session.host]);
		});

		it('attaching a terminal takes out the one shown there even if the caller did not detach it', async () => {
			const a = createTab(world, store, 'p1');
			const b = createTab(world, store, 'p1');
			await startTab(world, a);
			await startTab(world, b);
			a.session.attach(pane.container as unknown as HTMLElement);
			b.session.attach(pane.container as unknown as HTMLElement);
			expectHosts(pane, [b.session.host]);
		});
	});

	describe('detaching keeps the terminal', () => {
		it('keeps the shell, the xterm instance and the scrollback across detach and attach', async () => {
			const a = createTab(world, store, 'p1');
			const ptyA = await startTab(world, a);
			pane.showActive('p1');
			world.pty.print(ptyA, 'before\r\n');

			pane.showActive(null);
			world.pty.print(ptyA, 'while hidden\r\n');
			pane.showActive('p1');

			expect(a.term.disposed).toBe(false);
			expect(a.term.openCount).toBe(1);
			expect(world.pty.kills).toEqual([]);
			expect(a.session.status).toBe('running');
			expect(decoded(a.term)).toBe('before\r\nwhile hidden\r\n');
		});

		it('detaching a terminal twice is harmless', async () => {
			const a = createTab(world, store, 'p1');
			await startTab(world, a);
			a.session.detach();
			a.session.detach();
			expectHosts(pane, []);
			expect(a.term.disposed).toBe(false);
		});
	});

	describe('output stays with its terminal', () => {
		it('three tabs with distinct output each show only their own', async () => {
			const tabs = [createTab(world, store, 'p1'), createTab(world, store, 'p1'), createTab(world, store, 'p1')];
			const ptys: string[] = [];
			for (const tab of tabs) ptys.push(await startTab(world, tab));
			ptys.forEach((id, i) => world.pty.print(id, `tab-${i + 1}-output\r\n`));

			expect(decoded(tabs[0].term)).toBe('tab-1-output\r\n');
			expect(decoded(tabs[1].term)).toBe('tab-2-output\r\n');
			expect(decoded(tabs[2].term)).toBe('tab-3-output\r\n');
		});

		it('output of a hidden tab reaches that tab and no other', async () => {
			const a = createTab(world, store, 'p1');
			const b = createTab(world, store, 'p1');
			const ptyA = await startTab(world, a);
			const ptyB = await startTab(world, b);
			store.activate('p1', a.session.id);
			pane.showActive('p1');
			store.activate('p1', b.session.id);
			pane.showActive('p1');

			world.pty.print(ptyA, 'background\r\n');
			world.pty.print(ptyB, 'foreground\r\n');

			expect(decoded(a.term)).toBe('background\r\n');
			expect(decoded(b.term)).toBe('foreground\r\n');
		});

		it('the exit of one shell marks only that terminal as exited', async () => {
			const a = createTab(world, store, 'p1');
			const b = createTab(world, store, 'p1');
			const ptyA = await startTab(world, a);
			await startTab(world, b);
			world.pty.exit(ptyA, 0);

			expect(a.session.status).toBe('exited');
			expect(a.session.exitCode).toBe(0);
			expect(b.session.status).toBe('running');
			expect(decoded(b.term)).toBe('');
		});
	});

	describe('input stays with its terminal', () => {
		it('a keystroke is written to the shell of the terminal that received it', async () => {
			const a = createTab(world, store, 'p1');
			const b = createTab(world, store, 'p1');
			const ptyA = await startTab(world, a);
			const ptyB = await startTab(world, b);
			store.activate('p1', a.session.id);
			pane.showActive('p1');

			a.term.type('ls\r');
			store.activate('p1', b.session.id);
			pane.showActive('p1');
			b.term.type('pwd\r');
			await settle();

			expect(world.pty.writes).toEqual([
				{ id: ptyA, data: 'ls\r' },
				{ id: ptyB, data: 'pwd\r' }
			]);
		});

		it('input typed while the shell is still starting is not sent', async () => {
			const a = createTab(world, store, 'p1');
			pane.showActive('p1');
			a.term.type('early');
			await settle();
			expect(world.pty.writes).toEqual([]);
			expect(a.session.status).toBe('running');
		});
	});

	describe('resize stays with its terminal and is sent only on real changes', () => {
		it('a hidden terminal is never measured: no fit, no resize', async () => {
			const a = createTab(world, store, 'p1');
			await startTab(world, a);
			const hidden = new FakeEl();
			hidden.clientWidth = 0;
			hidden.clientHeight = 0;
			a.session.attach(hidden as unknown as HTMLElement);
			a.session.refit();
			a.session.requestRefit();
			await settle(200);

			expect(a.term.fitCalls).toBe(0);
			expect(a.term.cols).toBe(80);
			expect(world.pty.resizes).toEqual([]);
		});

		it('a shown terminal fits its own container and sends its size to its own shell', async () => {
			const a = createTab(world, store, 'p1');
			const b = createTab(world, store, 'p1');
			const ptyA = await startTab(world, a);
			await startTab(world, b);
			store.activate('p1', a.session.id);
			pane.showActive('p1');
			pane.container.clientWidth = 640;
			pane.container.clientHeight = 320;

			a.session.requestRefit();
			await settle(300);

			expect(a.term.cols).toBe(80);
			expect(a.term.rows).toBe(20);
			expect(world.pty.resizes.filter((r) => r.id === ptyA)).toEqual([{ id: ptyA, cols: 80, rows: 20 }]);
			expect(world.pty.resizes.every((r) => r.id === ptyA)).toBe(true);
		});

		it('a burst of container size changes becomes one refit', async () => {
			const a = createTab(world, store, 'p1');
			await startTab(world, a);
			pane.showActive('p1');
			await settle(300);
			const fitsBefore = a.term.fitCalls;
			for (let i = 0; i < 6; i++) {
				pane.container.clientWidth = 400 + i * 10;
				a.session.requestRefit();
			}
			await settle(300);
			expect(a.term.fitCalls - fitsBefore).toBe(1);
		});

		it('an unchanged size is not sent again', async () => {
			const a = createTab(world, store, 'p1');
			await startTab(world, a);
			pane.showActive('p1');
			await settle(300);
			const sentBefore = world.pty.resizes.length;

			a.session.refit();
			a.session.refit();
			a.session.requestRefit();
			await settle(300);

			expect(world.pty.resizes.length).toBe(sentBefore);
		});

		it('switching to a terminal refits it at its own container size', async () => {
			const a = createTab(world, store, 'p1');
			const b = createTab(world, store, 'p1');
			await startTab(world, a);
			const ptyB = await startTab(world, b);
			store.activate('p1', a.session.id);
			pane.showActive('p1');
			pane.container.clientWidth = 480;
			pane.container.clientHeight = 160;
			store.activate('p1', b.session.id);
			pane.showActive('p1');
			await settle(300);

			expect(b.term.cols).toBe(60);
			expect(b.term.rows).toBe(10);
			expect(world.pty.resizes.filter((r) => r.id === ptyB)).toEqual([{ id: ptyB, cols: 60, rows: 10 }]);
		});
	});

	describe('fonts', () => {
		it('a font that loads while a terminal is shown is measured again at once', async () => {
			const a = createTab(world, store, 'p1');
			await startTab(world, a);
			pane.showActive('p1');
			const writesBefore = a.term.fontWrites.length;

			a.session.fontsChanged();
			await settle(200);

			expect(a.term.fontWrites.slice(writesBefore)).toEqual(['', FONT]);
		});

		it('a hidden terminal is measured again on its next attach, not before', async () => {
			const a = createTab(world, store, 'p1');
			await startTab(world, a);
			pane.showActive('p1');
			pane.showActive(null);
			const writesBefore = a.term.fontWrites.length;

			a.session.fontsChanged();
			await settle(200);
			expect(a.term.fontWrites.length).toBe(writesBefore);

			pane.showActive('p1');
			expect(a.term.fontWrites.slice(writesBefore)).toEqual(['', FONT]);
		});

		it('a changed font family goes to the shown terminal at once and to a hidden one on its next attach', async () => {
			const a = createTab(world, store, 'p1');
			const b = createTab(world, store, 'p1');
			await startTab(world, a);
			await startTab(world, b);
			store.activate('p1', a.session.id);
			pane.showActive('p1');
			const hiddenWrites = b.term.fontWrites.length;
			world.font.value = 'Google Sans Code, ui-monospace';

			store.refreshTheme();

			expect(a.term.options.fontFamily).toBe('Google Sans Code, ui-monospace');
			expect(b.term.options.theme).toEqual(THEME);
			expect(b.term.fontWrites.length).toBe(hiddenWrites);

			store.activate('p1', b.session.id);
			pane.showActive('p1');
			expect(b.term.options.fontFamily).toBe('Google Sans Code, ui-monospace');
		});
	});

	describe('closing tabs', () => {
		it('closing an inactive tab leaves the shown one untouched', async () => {
			const a = createTab(world, store, 'p1');
			const b = createTab(world, store, 'p1');
			const ptyA = await startTab(world, a);
			const ptyB = await startTab(world, b);
			store.activate('p1', a.session.id);
			pane.showActive('p1');

			store.close('p1', b.session.id);
			world.pty.print(ptyB, 'late output\r\n');
			pane.showActive('p1');

			expect(store.activeId('p1')).toBe(a.session.id);
			expectHosts(pane, [a.session.host]);
			expect(a.term.disposed).toBe(false);
			expect(world.pty.kills).toEqual([ptyB]);
			expect(world.pty.kills).not.toContain(ptyA);
			expect(b.term.disposed).toBe(true);
			expect(decoded(a.term)).toBe('');
			expect(b.session.host.parentElement).toBeNull();
		});

		it('closing the shown tab shows its neighbour', async () => {
			const a = createTab(world, store, 'p1');
			const b = createTab(world, store, 'p1');
			const c = createTab(world, store, 'p1');
			for (const tab of [a, b, c]) await startTab(world, tab);
			store.activate('p1', b.session.id);
			pane.showActive('p1');

			store.close('p1', b.session.id);
			pane.showActive('p1');

			expect(store.activeId('p1')).toBe(c.session.id);
			expectHosts(pane, [c.session.host]);
			expect(c.term.disposed).toBe(false);
			expect(a.term.disposed).toBe(false);
		});

		it('closing the last tab leaves nothing shown', async () => {
			const a = createTab(world, store, 'p1');
			await startTab(world, a);
			pane.showActive('p1');

			store.close('p1', a.session.id);
			pane.showActive('p1');

			expect(store.activeId('p1')).toBeNull();
			expectHosts(pane, []);
			expect(a.term.disposed).toBe(true);
		});

		it('closing a tab whose shell is still starting kills that shell once it opens', async () => {
			const a = createTab(world, store, 'p1');
			a.session.start();
			const opened = world.pty.opens[0];
			store.close('p1', a.session.id);
			await settle();

			expect(world.pty.kills).toEqual([opened.id]);
		});

		it('after close, late shell output is ignored and nothing is written to the shell', async () => {
			const a = createTab(world, store, 'p1');
			const ptyA = await startTab(world, a);
			store.close('p1', a.session.id);

			expect(() => world.pty.print(ptyA, 'ghost\r\n')).not.toThrow();
			a.term.type('typed after close');
			await settle();

			expect(world.pty.writes).toEqual([]);
			expect(world.pty.kills).toEqual([ptyA]);
		});

		it('an unknown tab id is ignored by activate and close', async () => {
			const a = createTab(world, store, 'p1');
			await startTab(world, a);
			store.activate('p1', 'no-such-tab');
			store.close('p1', 'no-such-tab');
			expect(store.activeId('p1')).toBe(a.session.id);
			expect(store.list('p1')).toHaveLength(1);
			expect(store.list('p1')[0]).toBe(a.session);
		});
	});

	describe('projects', () => {
		it('each project keeps its own terminals and its own shown tab', async () => {
			const a = createTab(world, store, 'p1');
			const c = createTab(world, store, 'p2');
			const ptyA = await startTab(world, a);
			const ptyC = await startTab(world, c);

			pane.showActive('p1');
			expectHosts(pane, [a.session.host]);
			pane.showActive('p2');
			expectHosts(pane, [c.session.host]);
			expect(a.session.host.parentElement).toBeNull();

			world.pty.print(ptyA, 'project one\r\n');
			world.pty.print(ptyC, 'project two\r\n');
			expect(decoded(a.term)).toBe('project one\r\n');
			expect(decoded(c.term)).toBe('project two\r\n');
		});

		it('switching back to a project shows its terminal again, with its state', async () => {
			const a = createTab(world, store, 'p1');
			const ptyA = await startTab(world, a);
			pane.showActive('p1');
			world.pty.print(ptyA, 'kept\r\n');
			pane.showActive('p2');
			pane.showActive('p1');

			expectHosts(pane, [a.session.host]);
			expect(decoded(a.term)).toBe('kept\r\n');
			expect(world.pty.kills).toEqual([]);
		});

		it('input typed in a project terminal goes to that project only', async () => {
			const a = createTab(world, store, 'p1');
			const c = createTab(world, store, 'p2');
			const ptyA = await startTab(world, a);
			const ptyC = await startTab(world, c);
			pane.showActive('p2');
			pane.showActive('p1');

			a.term.type('echo one\r');
			c.term.type('echo two\r');
			await settle();

			expect(world.pty.writes).toEqual([
				{ id: ptyA, data: 'echo one\r' },
				{ id: ptyC, data: 'echo two\r' }
			]);
		});

		it('a project with no terminal shows nothing and leaves the others alive', async () => {
			const a = createTab(world, store, 'p1');
			await startTab(world, a);
			pane.showActive('p1');
			pane.showActive('p9');

			expectHosts(pane, []);
			expect(store.list('p9')).toEqual([]);
			expect(a.term.disposed).toBe(false);
		});

		it('a terminal opened for one project is not listed under another', () => {
			const a = createTab(world, store, 'p1');
			expect(store.list('p2')).toHaveLength(0);
			expect(store.list('p1')).toHaveLength(1);
			expect(store.list('p1')[0]).toBe(a.session);
		});

		it('seeds the first terminal of a project only once', () => {
			store.seed('p1', null);
			store.seed('p1', null);
			expect(store.list('p1')).toHaveLength(1);
		});
	});

	describe('shell lifecycle', () => {
		it('restarts the shell in the same terminal after it exited', async () => {
			const a = createTab(world, store, 'p1');
			const first = await startTab(world, a);
			world.pty.exit(first, 1);
			expect(a.session.status).toBe('exited');

			a.session.restart();
			await settle();

			expect(world.pty.opens.length).toBe(2);
			expect(a.session.status).toBe('running');
			expect(a.term.disposed).toBe(false);
			expect(a.session.exitCode).toBeNull();
		});

		it('a failed input write is reported, not thrown into xterm', async () => {
			const a = createTab(world, store, 'p1');
			await startTab(world, a);
			world.pty.write = () => Promise.reject(new Error('pipe closed'));
			expect(() => a.term.type('x')).not.toThrow();
			await settle();
			expect(world.errors).toHaveLength(1);
		});

		it('dispose is idempotent and kills the shell once', async () => {
			const a = createTab(world, store, 'p1');
			const ptyA = await startTab(world, a);
			a.session.dispose();
			a.session.dispose();
			expect(world.pty.kills).toEqual([ptyA]);
			expect(a.term.disposed).toBe(true);
		});
	});
});

