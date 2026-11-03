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

import { t } from '$lib/i18n/i18n.svelte';
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
		get label() { return t('common.conversation') },
		items: [
			{ view: 'session', get label() { return t('common.session') } },
			{ view: 'tools', get label() { return t('common.tools') } },
			{ view: 'mcp', get label() { return t('common.mcpServers') } },
			{ view: 'config', get label() { return t('common.configuration') } }
		]
	},
	{
		get label() { return t('common.agents') },
		items: [
			{ view: 'agents', get label() { return t('common.agentProfiles') } },
			{ view: 'jobs', get label() { return t('common.backgroundJobs') } }
		]
	},
	{
		get label() { return t('context.contextAndMemory') },
		items: [
			{ view: 'context', get label() { return t('common.context') } },
			{ view: 'cost', get label() { return t('common.usage') } },
			{ view: 'memory', get label() { return t('common.memory') } },
			{ view: 'search', get label() { return t('common.codeSearch') } }
		]
	}
];

/** Every view offered by the menu, in menu order. */
export function adkAccessViews(groups: readonly AdkAccessGroup[] = ADK_ACCESS_GROUPS): InspectorView[] {
	return groups.flatMap((group) => group.items.map((item) => item.view));
}
