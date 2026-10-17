// The terminals of every project: which ones exist, which one is shown for each
// project. The sessions live here, not in the component: the pane unmounts when it
// is closed, but the shells, their scrollback and their xterm instances survive,
// and the component attaches the active one again on mount. Each terminal belongs
// to the project it was opened for. Owner: Haiku F3 (Sprint 14.3, terminals).

import { SvelteMap, SvelteSet } from 'svelte/reactivity';
import { TerminalSession, type TerminalDeps } from './terminal-session.svelte';

export class TerminalStore {
	#sessions = new SvelteMap<string, TerminalSession[]>();
	#active = new SvelteMap<string, string>();
	#seeded = new SvelteSet<string>();
	readonly #deps: TerminalDeps;

	constructor(deps: TerminalDeps) {
		this.#deps = deps;
	}

	list(projectId: string): TerminalSession[] {
		return this.#sessions.get(projectId) ?? [];
	}

	activeId(projectId: string): string | null {
		return this.#active.get(projectId) ?? null;
	}

	/** Opens the first terminal of a project, once per application run. */
	seed(projectId: string, sessionId: string | null): void {
		if (this.#seeded.has(projectId)) return;
		this.#seeded.add(projectId);
		this.create(projectId, sessionId);
	}

	create(projectId: string, sessionId: string | null): TerminalSession {
		const session = new TerminalSession(projectId, sessionId, this.#deps);
		this.#sessions.set(projectId, [...this.list(projectId), session]);
		this.#active.set(projectId, session.id);
		return session;
	}

	/** Makes one terminal of the project the shown one. Unknown ids are ignored. */
	activate(projectId: string, id: string): void {
		if (this.list(projectId).some((s) => s.id === id)) this.#active.set(projectId, id);
	}

	/** Kills and forgets a terminal. If it was shown, the neighbour becomes the shown one. */
	close(projectId: string, id: string): void {
		const current = this.list(projectId);
		const index = current.findIndex((s) => s.id === id);
		if (index < 0) return;
		const closed = current[index];
		const remaining = current.filter((s) => s.id !== id);
		this.#sessions.set(projectId, remaining);
		if (this.activeId(projectId) === id) {
			const next = remaining[Math.min(index, remaining.length - 1)];
			if (next) this.#active.set(projectId, next.id);
			else this.#active.delete(projectId);
		}
		closed.dispose();
	}

	/** Applies the current light or dark colours to every open terminal. */
	refreshTheme(): void {
		for (const sessions of this.#sessions.values()) {
			for (const session of sessions) session.refreshLook();
		}
	}

	/** Fonts changed: every terminal measures its glyphs again before it is shown. */
	refreshFonts(): void {
		for (const sessions of this.#sessions.values()) {
			for (const session of sessions) session.fontsChanged();
		}
	}
}
