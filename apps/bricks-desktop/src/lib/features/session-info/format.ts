// Labels and pure helpers of the session inspectors (French, no state, no IPC).
// Owner: Haiku 6 (Sprint 14.1).

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

/** An action of the approval policy, written as a setting ("Autoriser", ...). */
export function ruleLabel(action: string): string {
	switch (action) {
		case 'allow':
			return 'Autoriser';
		case 'ask':
			return 'Demander';
		case 'deny':
			return 'Refuser';
		default:
			return action;
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

/** Levels of the approval policy, in the order the configuration lists them. */
export const RULE_LEVELS: ReadonlyArray<{ key: string; label: string }> = [
	{ key: 'none', label: 'Aucune' },
	{ key: 'read_only', label: 'Lecture' },
	{ key: 'write', label: 'Écriture' },
	{ key: 'execute', label: 'Exécution' },
	{ key: 'dangerous', label: 'Dangereux' }
];

export interface RulesView {
	/** Action for tools no other rule covers. */
	defaultAction: string;
	levels: Array<{ key: string; label: string; action: string }>;
	/** Per-tool overrides, by tool name. */
	tools: Array<{ name: string; action: string }>;
}

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/**
 * The approval policy (`ApprovalRules` as JSON) read for display. `null`
 * when the value does not have the expected shape.
 */
export function rulesView(value: unknown): RulesView | null {
	if (!isRecord(value)) return null;
	const action = (v: unknown): string | null => (typeof v === 'string' ? v : null);
	const defaultAction = action(value.default);
	if (defaultAction === null) return null;
	const levels: RulesView['levels'] = [];
	for (const level of RULE_LEVELS) {
		const found = action(value[level.key]);
		if (found === null) return null;
		levels.push({ key: level.key, label: level.label, action: found });
	}
	const tools: RulesView['tools'] = [];
	if (isRecord(value.tools)) {
		for (const [name, raw] of Object.entries(value.tools)) {
			const found = action(raw);
			if (found !== null) tools.push({ name, action: found });
		}
	}
	tools.sort((a, b) => a.name.localeCompare(b.name, 'fr'));
	return { defaultAction, levels, tools };
}
