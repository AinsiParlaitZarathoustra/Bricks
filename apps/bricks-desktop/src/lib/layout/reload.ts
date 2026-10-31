// Reloading the page drops the live state of every conversation: the built
// app refuses it (the native menu's entry is removed in Rust, webview_menu.rs).
// A development build keeps it. Pure: unit-tested (tests/unit/reload.test.ts).

/** F5, Cmd+R or Ctrl+R, with or without Shift. */
export function isReloadShortcut(e: Pick<KeyboardEvent, 'key' | 'metaKey' | 'ctrlKey' | 'altKey'>): boolean {
	if (e.key === 'F5') return true;
	return (e.metaKey || e.ctrlKey) && !e.altKey && e.key.toLowerCase() === 'r';
}

/** Window handler of the built app: the shortcut does nothing more than what a field made of it. */
export function blockReload(e: KeyboardEvent): void {
	if (isReloadShortcut(e)) e.preventDefault();
}
