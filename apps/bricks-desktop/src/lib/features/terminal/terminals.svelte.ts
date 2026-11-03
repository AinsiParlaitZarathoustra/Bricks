// Live wiring of the terminal feature: xterm, the PTY commands and the app state
// plug into the store and the session rules (terminal-store / terminal-session,
// tested with fakes). The bottom pane uses the `terminals` instance below.

import { t } from '$lib/i18n/i18n.svelte';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { errorMessage, ptyKill, ptyOpen, ptyResize, ptyWrite } from '$lib/ipc/client';
import { app } from '$lib/state/app.svelte';
import { SCROLLBACK, type TerminalDeps, type TerminalEngine } from './terminal-session.svelte';
import { TerminalStore } from './terminal-store.svelte';
import { terminalFont, terminalFontSize, terminalTheme } from './terminal-theme';

const liveDeps: TerminalDeps = {
	createEngine({ font, theme }): TerminalEngine {
		// xterm's own screen-reader strings are static and shared: each new terminal sets them.
		Terminal.strings.promptLabel = t('terminal.terminalInput');
		Terminal.strings.tooMuchOutput = t('terminal.tooMuchOutputLineByLine');
		const term = new Terminal({
			// Exposes the output to screen readers (NVDA, VoiceOver).
			screenReaderMode: true,
			scrollback: SCROLLBACK,
			cursorBlink: true,
			fontSize: terminalFontSize(),
			lineHeight: 1.2,
			fontFamily: font,
			theme
		});
		const fit = new FitAddon();
		term.loadAddon(fit);
		return {
			term,
			fit() {
				fit.fit();
			}
		};
	},
	createHost() {
		const host = document.createElement('div');
		host.className = 'h-full w-full';
		return host;
	},
	pty: { open: ptyOpen, write: ptyWrite, resize: ptyResize, kill: ptyKill },
	font: terminalFont,
	theme: terminalTheme,
	errorMessage,
	onError: (e) => app.fail(e)
};

export const terminals = new TerminalStore(liveDeps);

// Google Sans Code may finish loading after a terminal measured its glyphs: measure again.
if (typeof document !== 'undefined' && document.fonts) {
	document.fonts.addEventListener('loadingdone', () => terminals.refreshFonts());
	void document.fonts.ready.then(() => terminals.refreshFonts());
}
