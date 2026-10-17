// Discoverable access to the ADK inspectors that already exist, from the right
// panel's « Plus » menu. Pure data: the menu itself lives in ContextPanel.svelte.
// Each entry opens a view owned by its feature folder through `ui.open(view)`.
//
// Rules, in one place:
// - A view appears once. Actions are not duplicated across panels: the context
//   panel only links to inspectors; compaction, clearing, stop and apply stay in
//   the views themselves.
// - Labels are the titles the inspector dialog shows, so the user recognises them.
// - Every entry reads the shown conversation: the menu is disabled without one.

import type { InspectorView } from '$lib/state/ui.svelte';

export interface AdkAccessItem {
	view: InspectorView;
	/** Same wording as the inspector dialog title (InspectorHost). */
	label: string;
}

export interface AdkAccessGroup {
	label: string;
	items: readonly AdkAccessItem[];
}

export const ADK_ACCESS_GROUPS: readonly AdkAccessGroup[] = [
	{
		label: 'Conversation',
		items: [
			{ view: 'session', label: 'Session' },
			{ view: 'tools', label: 'Outils' },
			{ view: 'mcp', label: 'Serveurs MCP' },
			{ view: 'config', label: 'Configuration' }
		]
	},
	{
		label: 'Agents',
		items: [
			{ view: 'agent_instances', label: 'Sous-agents' },
			{ view: 'agents', label: 'Profils d’agents' },
			{ view: 'jobs', label: 'Tâches de fond' }
		]
	},
	{
		label: 'Contexte et mémoire',
		items: [
			{ view: 'context', label: 'Contexte' },
			{ view: 'cost', label: 'Consommation' },
			{ view: 'memory', label: 'Mémoire' },
			{ view: 'search', label: 'Recherche dans le code' }
		]
	}
];

/** Every view offered by the menu, in menu order. */
export function adkAccessViews(groups: readonly AdkAccessGroup[] = ADK_ACCESS_GROUPS): InspectorView[] {
	return groups.flatMap((group) => group.items.map((item) => item.view));
}
