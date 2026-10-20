// The timeline a reader test changes: a reactive array, as the items of a
// conversation are. A `.svelte.ts` module, so that `$state` compiles here.

import type { TimelineItem } from '../../src/lib/state/conversation.svelte';

/** An empty timeline whose changes the reader's derived values follow. */
export function timeline(): TimelineItem[] {
	const items = $state<TimelineItem[]>([]);
	return items;
}
