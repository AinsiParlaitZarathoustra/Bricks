// Labels and pure helpers of the session inspectors (French, no state, no IPC).

import { t, localeTag } from '$lib/i18n/i18n.svelte';
import type { BadgeVariant } from '$lib/components/ui/badge';
import type { Activity } from '$lib/ipc/engine';

/** Permission levels as the engine names them (`SessionToolInfo.level`). */
const LEVEL_LABELS: Record<string, string> = {
	get none() { return t('common.none') },
	get read_only() { return t('common.reading') },
	get write() { return t('common.write') },
	get execute() { return t('common.run') },
	get dangerous() { return t('sessionInfo.dangerous') },
	get forbidden() { return t('sessionInfo.forbidden') }
};

export function levelLabel(level: string): string {
	return LEVEL_LABELS[level] ?? level;
}

/** Decision of an outcome for a tool (`SessionToolInfo.decision`). */
export function decisionLabel(decision: string): string {
	switch (decision) {
		case 'allow':
			return t('common.allowed');
		case 'ask':
			return t('sessionInfo.onRequest');
		case 'deny':
			return t('common.refused2');
		default:
			return decision;
	}
}

export function decisionVariant(decision: string): BadgeVariant {
	switch (decision) {
		case 'allow':
			return 'secondary';
		case 'deny':
			return 'destructive';
		default:
			return 'outline';
	}
}

/** What the engine is doing (`Snapshot.activity`). */
export function activityLabel(activity: Activity): string {
	switch (activity.state) {
		case 'idle':
			return t('sessionInfo.idle');
		case 'running':
			return t('sessionInfo.runInProgress');
		case 'busy':
			return t('sessionInfo.busy', { what: activity.what });
	}
}

export interface McpStatusView {
	label: string;
	detail: string | null;
	tone: 'success' | 'error' | 'muted';
}

/** An MCP server state as the engine reports it (`Connected`, `Error("…")`, ...). */
export function mcpStatusView(status: string): McpStatusView {
	if (status === 'Connected') return { label: t('sessionInfo.connected'), detail: null, tone: 'success' };
	if (status === 'Disconnected') return { label: t('sessionInfo.disconnected'), detail: null, tone: 'muted' };
	const error = /^Error\((.*)\)$/s.exec(status);
	if (error) return { label: t('common.error'), detail: unquote(error[1]), tone: 'error' };
	if (status.startsWith('not connected yet')) {
		return { label: t('sessionInfo.notConnectedYet'), detail: t('sessionInfo.connectsOnTheFirstRun'), tone: 'muted' };
	}
	return { label: status, detail: null, tone: 'muted' };
}

/** The text of a Debug-quoted string (`"…"`), or the input when it is not one. */
function unquote(text: string): string {
	try {
		const value: unknown = JSON.parse(text);
		return typeof value === 'string' ? value : text;
	} catch {
		return text;
	}
}

/** Whether a tool matches the filter (name or description, case-insensitive). */
export function matchesTool(tool: { name: string; description: string }, filter: string): boolean {
	const needle = filter.trim().toLocaleLowerCase(localeTag());
	if (!needle) return true;
	return (
		tool.name.toLocaleLowerCase(localeTag()).includes(needle) ||
		tool.description.toLocaleLowerCase(localeTag()).includes(needle)
	);
}
