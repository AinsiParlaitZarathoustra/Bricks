// Links from the changes panel to a sub-agent ChangeSet inspector (`ChangesetView`).
// Pure helpers: which distinct changesets a file's edits belong to, and the label
// of the link.

import { t } from '$lib/i18n/i18n.svelte';
import { shortId } from '$lib/features/changes/format';

/** The distinct changesets of some operations, in order of first appearance. */
export function changesetIdsOf(operations: readonly { changesetId: string | null }[]): string[] {
	const ids: string[] = [];
	for (const op of operations) {
		const id = op.changesetId;
		if (id && !ids.includes(id)) ids.push(id);
	}
	return ids;
}

/** Link label. A file touched by several changesets adds the short id of each link. */
export function changesetLinkLabel(ids: readonly string[], id: string): string {
	const base = t('catalog.viewIsolatedChanges');
	return ids.length > 1 ? `${base} · ${shortId(id)}` : base;
}
