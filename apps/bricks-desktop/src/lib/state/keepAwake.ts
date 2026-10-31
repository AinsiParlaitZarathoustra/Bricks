// When the computer must stay awake: always while Bricks is open (Paramètres ›
// Général), or while a conversation that asked for it runs (menu Plus). Pure:
// unit-tested (tests/unit/keep-awake.test.ts).

export function keepAwakeWanted(
	always: boolean,
	conversations: Iterable<{ keepAwake: boolean; running: boolean }>
): boolean {
	if (always) return true;
	for (const c of conversations) if (c.keepAwake && c.running) return true;
	return false;
}
