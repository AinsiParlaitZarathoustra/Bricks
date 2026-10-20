// One light line per tool call, and the one-line trace of a resolved approval.
// Shared by the timeline and the sub-agent inspector. Pure functions only.

import { DECIDED_BY_LABEL, DECISION_LABEL, formatDuration } from '../format';
import type { DecidedBy, Decision } from '$lib/ipc/engine';
import type { ToolApproval } from '$lib/ipc/bindings/ToolApproval';

/** What the line says about a call. */
export type ToolLineState = 'running' | 'done' | 'failed' | 'interrupted';

export const TOOL_LINE_LABEL: Record<ToolLineState, string> = {
	running: 'En cours',
	done: 'Terminé',
	failed: 'Échec',
	interrupted: 'Interrompu'
};

/** The facts of a call that the line depends on (a timeline tool or a sub-agent tool). */
export interface ToolCallFacts {
	status: 'running' | 'ok' | 'error';
	durationMs?: number;
	output?: string;
}

/**
 * State of a call. `interrupted` is true when the run (or sub-agent) that made the
 * call ended by cancellation or interruption. The reducer marks the calls still
 * running at the end of a run as errors without a result: under such a run, those
 * calls are interruptions, not failures. A call with a result stays done or failed.
 */
export function toolLineState(call: ToolCallFacts, interrupted: boolean): ToolLineState {
	if (call.status === 'ok') return 'done';
	if (call.status === 'running') return interrupted ? 'interrupted' : 'running';
	if (interrupted && call.durationMs === undefined && call.output === undefined) {
		return 'interrupted';
	}
	return 'failed';
}

/** Compact elapsed time; `null` when the engine gave no valid duration. */
export function formatToolDuration(ms: number | undefined): string | null {
	if (ms === undefined || !Number.isFinite(ms) || ms < 0) return null;
	return formatDuration(ms);
}

/** The trace that replaces a resolved approval: `Autorisé · Bash · par vous`. */
export function approvalTrace(tool: string, resolved: { decision: Decision; by: DecidedBy }): string {
	return `${DECISION_LABEL[resolved.decision]} · ${tool} · ${DECIDED_BY_LABEL[resolved.by]}`;
}

/**
 * Whether the card offers « Autoriser pour la session ». Not under « Toujours demander »
 * (always_ask): a session grant is ignored in that mode, and would apply again later.
 */
export function sessionGrantOffered(mode: ToolApproval): boolean {
	return mode !== 'always_ask';
}
