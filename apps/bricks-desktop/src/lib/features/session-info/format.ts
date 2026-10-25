// Labels and pure helpers of the session inspectors (French, no state, no IPC).

import type { BadgeVariant } from '$lib/components/ui/badge';
import type { Activity } from '$lib/ipc/engine';

/** Permission levels as the engine names them (`SessionToolInfo.level`). */
const LEVEL_LABELS: Record<string, string> = {
	none: 'Aucune',
	read_only: 'Lecture',
	write: 'Écriture',
	execute: 'Exécution',
	dangerous: 'Dangereux',
	forbidden: 'Interdit'
};

export function levelLabel(level: string): string {
	return LEVEL_LABELS[level] ?? level;
}

/** Decision of an outcome for a tool (`SessionToolInfo.decision`). */
export function decisionLabel(decision: string): string {
	switch (decision) {
		case 'allow':
			return 'Autorisé';
		case 'ask':
			return 'Sur demande';
		case 'deny':
			return 'Refusé';
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
			return 'Inactive';
		case 'running':
			return 'Exécution en cours';
		case 'busy':
			return `Occupée : ${activity.what}`;
	}
}

export interface McpStatusView {
	label: string;
	detail: string | null;
	tone: 'success' | 'error' | 'muted';
}

/** An MCP server state as the engine reports it (`Connected`, `Error("…")`, ...). */
export function mcpStatusView(status: string): McpStatusView {
	if (status === 'Connected') return { label: 'Connecté', detail: null, tone: 'success' };
	if (status === 'Disconnected') return { label: 'Déconnecté', detail: null, tone: 'muted' };
	const error = /^Error\((.*)\)$/s.exec(status);
	if (error) return { label: 'Erreur', detail: unquote(error[1]), tone: 'error' };
	if (status.startsWith('not connected yet')) {
		return { label: 'Pas encore connecté', detail: 'Connexion à la première exécution.', tone: 'muted' };
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
	const needle = filter.trim().toLocaleLowerCase('fr');
	if (!needle) return true;
	return (
		tool.name.toLocaleLowerCase('fr').includes(needle) ||
		tool.description.toLocaleLowerCase('fr').includes(needle)
	);
}
