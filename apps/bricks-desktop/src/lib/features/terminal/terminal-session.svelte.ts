// One terminal tab: an xterm instance, the shell running on its PTY and the
// element that shows it. Attaching moves that element into a container and
// detaching takes it out again; neither touches the shell, the xterm state or
// the scrollback. Only `dispose()` (an explicit close) kills the PTY and frees
// xterm. xterm and the PTY commands are injected through `TerminalDeps`, so the
// rules here are testable without a window. Owner: Haiku F3 (Sprint 14.3, terminals).

import type { IDisposable, ITerminalOptions, ITheme } from '@xterm/xterm';
import type { PtyEvent } from '../../ipc/bindings/PtyEvent';
import type { PtyInfo } from '../../ipc/bindings/PtyInfo';
import type { PtyOpenRequest } from '../../ipc/bindings/PtyOpenRequest';

/** Lines kept per terminal. */
export const SCROLLBACK = 5000;
/** Quiet time before a size change is sent to the PTY. */
const RESIZE_DELAY_MS = 120;
/** Quiet time before a container size change refits the terminal. */
const REFIT_DELAY_MS = 50;

export type TerminalStatus = 'idle' | 'starting' | 'running' | 'exited' | 'error';

/** The part of xterm's `Terminal` that a session drives. */
export interface XtermPort {
	readonly cols: number;
	readonly rows: number;
	options: Pick<ITerminalOptions, 'fontFamily' | 'theme'>;
	open(parent: HTMLElement): void;
	write(data: string | Uint8Array): void;
	focus(): void;
	dispose(): void;
	onData(listener: (data: string) => void): IDisposable;
	onResize(listener: (size: { cols: number; rows: number }) => void): IDisposable;
}

/** An xterm instance with its fit addon. The instance is not open yet. */
export interface TerminalEngine {
	readonly term: XtermPort;
	/** Sets cols and rows from the element the terminal is open in. */
	fit(): void;
}

/** The PTY commands of the desktop engine (`src/lib/ipc/client.ts`). */
export interface PtyPort {
	open(request: PtyOpenRequest, onEvent: (event: PtyEvent) => void): Promise<PtyInfo>;
	write(ptyId: string, data: string): Promise<void>;
	resize(ptyId: string, cols: number, rows: number): Promise<void>;
	kill(ptyId: string): Promise<void>;
}

export interface TerminalDeps {
	/** Builds an xterm instance with its fit addon, using the given colours and font. */
	createEngine(look: { font: string; theme: ITheme }): TerminalEngine;
	/** Element the xterm instance draws into. */
	createHost(): HTMLElement;
	pty: PtyPort;
	/** Font family of the terminal, read from the CSS tokens. */
	font(): string;
	/** Colours of the terminal, read from the CSS tokens of the current mode. */
	theme(): ITheme;
	/** Human-readable text of an error thrown by a command. */
	errorMessage(error: unknown): string;
	/** Reports an action that failed while the user was typing. */
	onError(error: unknown): void;
}

function decodeBase64(data: string): Uint8Array {
	const binary = atob(data);
	const bytes = new Uint8Array(binary.length);
	for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
	return bytes;
}

/** An element takes room on screen only when it is shown: a hidden one measures 0. */
function isShown(element: HTMLElement): boolean {
	return element.clientWidth > 0 && element.clientHeight > 0;
}

/** The terminal shown in each container. A container holds at most one at a time. */
const shownIn = new WeakMap<HTMLElement, TerminalSession>();

export class TerminalSession {
	readonly id = crypto.randomUUID();
	readonly projectId: string;
	readonly sessionId: string | null;
	/** Element the xterm instance draws into. Moved between containers, never recreated. */
	readonly host: HTMLElement;
	status = $state<TerminalStatus>('idle');
	/** Working directory the shell started in (from the PTY). */
	cwd = $state('');
	exitCode = $state<number | null>(null);
	error = $state<string | null>(null);
	readonly #deps: TerminalDeps;
	readonly #engine: TerminalEngine;
	#ptyId: string | null = null;
	/** Size last given to the PTY (at open or by resize). Only real changes are sent. */
	#size: { cols: number; rows: number } | null = null;
	#container: HTMLElement | null = null;
	#opened = false;
	/** The font must be applied (and the glyphs measured) on the next attach, not while hidden. */
	#fontStale = false;
	/** Set by dispose(): the session can no longer be shown, started or written to. */
	#closing = false;
	#resizeTimer: ReturnType<typeof setTimeout> | null = null;
	#refitTimer: ReturnType<typeof setTimeout> | null = null;

	constructor(projectId: string, sessionId: string | null, deps: TerminalDeps) {
		this.projectId = projectId;
		this.sessionId = sessionId;
		this.#deps = deps;
		this.host = deps.createHost();
		this.#engine = deps.createEngine({ font: deps.font(), theme: deps.theme() });
		this.#engine.term.onData((data) => this.#input(data));
		this.#engine.term.onResize(() => this.#scheduleResize());
	}

	/**
	 * Shows the terminal inside `container`. The terminal shown there before is
	 * taken out first, so a container never holds two terminals.
	 */
	attach(container: HTMLElement): void {
		if (this.#closing) return;
		const shown = shownIn.get(container);
		if (shown && shown !== this) shown.detach();
		if (this.#container !== container) this.detach();
		this.#container = container;
		shownIn.set(container, this);
		if (this.host.parentElement !== container) container.appendChild(this.host);
		if (!this.#opened) {
			this.#engine.term.open(this.host);
			this.#opened = true;
		}
		if (this.#fontStale) this.#applyFont(true);
		this.refit();
	}

	/**
	 * Takes the terminal out of its container. The shell, the xterm state and the
	 * scrollback are kept: the terminal can be attached again later.
	 */
	detach(): void {
		const container = this.#container;
		if (container && shownIn.get(container) === this) shownIn.delete(container);
		if (this.#refitTimer) {
			clearTimeout(this.#refitTimer);
			this.#refitTimer = null;
		}
		this.host.remove();
		this.#container = null;
	}

	/** Refits now, if the terminal is shown. A hidden terminal is never measured. */
	refit(): void {
		const container = this.#container;
		if (!this.#opened || !container || this.host.parentElement !== container) return;
		if (!isShown(container)) return;
		this.#engine.fit();
	}

	/** Refits once the size settles (ResizeObserver bursts become one refit). */
	requestRefit(): void {
		if (this.#refitTimer) clearTimeout(this.#refitTimer);
		this.#refitTimer = setTimeout(() => {
			this.#refitTimer = null;
			this.refit();
		}, REFIT_DELAY_MS);
	}

	/**
	 * Applies the colours of the current light or dark mode and the terminal font.
	 * A hidden terminal takes the new font on its next attach: xterm measures the
	 * glyphs when the font changes, and a hidden terminal is never measured.
	 */
	refreshLook(): void {
		if (this.#closing) return;
		this.#engine.term.options.theme = this.#deps.theme();
		if (this.#container) this.#applyFont(false);
		else this.#fontStale = true;
	}

	/**
	 * Fonts finished loading: the glyph metrics measured before may be stale. A shown
	 * terminal measures again now; a hidden one on its next attach.
	 */
	fontsChanged(): void {
		if (this.#closing) return;
		if (this.#container) {
			this.#applyFont(true);
			this.requestRefit();
		} else {
			this.#fontStale = true;
		}
	}

	focus(): void {
		this.#engine.term.focus();
	}

	/** Starts the shell in the project folder. Does nothing while one runs. */
	start(): void {
		if (this.#closing || this.status === 'starting' || this.status === 'running') return;
		this.status = 'starting';
		this.error = null;
		this.exitCode = null;
		const { term } = this.#engine;
		const size = { cols: term.cols, rows: term.rows };
		this.#size = size;
		this.#deps.pty
			.open(
				{ project_id: this.projectId, session_id: this.sessionId, cols: size.cols, rows: size.rows },
				(event) => this.#onEvent(event),
			)
			.then((info) => {
				this.#ptyId = info.id;
				this.cwd = info.cwd;
				if (this.#closing) {
					void this.#deps.pty.kill(info.id).catch(() => {});
					return;
				}
				// Output or exit may have arrived before this reply.
				if (this.status === 'starting') this.status = 'running';
				this.#sendSize(term.cols, term.rows);
			})
			.catch((e: unknown) => {
				if (this.#closing) return;
				this.status = 'error';
				this.error = this.#deps.errorMessage(e);
			});
	}

	/** Starts a new shell after the previous one exited or failed. */
	restart(): void {
		if (this.#closing) return;
		if (this.status !== 'exited' && this.status !== 'error') return;
		this.#ptyId = null;
		this.#engine.term.write('\r\n');
		this.start();
	}

	/** Kills the shell and releases xterm. The session cannot be used after. */
	dispose(): void {
		if (this.#closing) return;
		this.#closing = true;
		if (this.#resizeTimer) clearTimeout(this.#resizeTimer);
		this.#resizeTimer = null;
		this.detach();
		const id = this.#ptyId;
		this.#ptyId = null;
		if (id) void this.#deps.pty.kill(id).catch(() => {});
		this.#engine.term.dispose();
	}

	/**
	 * Sets the terminal font from the CSS tokens. With `remeasure`, the glyphs are
	 * measured again even when the family is unchanged: xterm measures only when a
	 * font option changes, so the same family goes through an empty one first.
	 */
	#applyFont(remeasure: boolean): void {
		this.#fontStale = false;
		const { term } = this.#engine;
		const family = this.#deps.font();
		if (remeasure && term.options.fontFamily === family) term.options.fontFamily = '';
		if (term.options.fontFamily !== family) term.options.fontFamily = family;
	}

	#onEvent(event: PtyEvent): void {
		if (this.#closing) return;
		if (event.kind === 'data') {
			this.#engine.term.write(decodeBase64(event.data));
			return;
		}
		this.status = 'exited';
		this.exitCode = event.code;
		this.#ptyId = null;
		const code = event.code === null ? '' : ` (code ${event.code})`;
		this.#engine.term.write(`\r\n\x1b[2m[Processus terminé${code}]\x1b[0m\r\n`);
	}

	#input(data: string): void {
		const id = this.#ptyId;
		if (!id || this.status !== 'running') return;
		this.#deps.pty.write(id, data).catch((e: unknown) => this.#deps.onError(e));
	}

	#scheduleResize(): void {
		if (this.#resizeTimer) clearTimeout(this.#resizeTimer);
		this.#resizeTimer = setTimeout(() => {
			this.#resizeTimer = null;
			const { term } = this.#engine;
			this.#sendSize(term.cols, term.rows);
		}, RESIZE_DELAY_MS);
	}

	#sendSize(cols: number, rows: number): void {
		const id = this.#ptyId;
		if (!id || this.status !== 'running') return;
		const last = this.#size;
		if (last && last.cols === cols && last.rows === rows) return;
		this.#size = { cols, rows };
		// A shell that already ended has no size left to change.
		this.#deps.pty.resize(id, cols, rows).catch(() => {});
	}
}
