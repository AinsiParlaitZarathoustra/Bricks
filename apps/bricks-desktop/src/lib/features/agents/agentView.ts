// Sub-agent presentation: state mapping, durations and French labels.
// Pure helpers, no component state.

import type { ChangeSetState, InstanceState, Termination } from '$lib/ipc/engine';
import { TERMINAL_AGENT_STATES } from '$lib/ipc/engine';
import type { AgentInfo } from '$lib/state/conversation.svelte';
import type { StatusState } from '$lib/layout/StatusDot.svelte';
import type { BadgeVariant } from '$lib/components/ui/badge';

export interface AgentStateView {
	dot: StatusState;
	label: string;
}

const STATE_VIEW: Record<InstanceState, AgentStateView> = {
	created: { dot: 'waiting', label: 'Créé' },
	queued: { dot: 'waiting', label: 'En file d’attente' },
	waiting_admission: { dot: 'waiting', label: 'En attente d’admission' },
	starting: { dot: 'running', label: 'Démarrage' },
	running: { dot: 'running', label: 'En cours' },
	cancelling: { dot: 'running', label: 'Annulation en cours' },
	completed: { dot: 'success', label: 'Terminé' },
	incomplete: { dot: 'waiting', label: 'Incomplet' },
	failed: { dot: 'error', label: 'Échec' },
	interrupted: { dot: 'error', label: 'Interrompu' },
	cancelled: { dot: 'cancelled', label: 'Annulé' }
};

export function agentStateView(state: InstanceState): AgentStateView {
	return STATE_VIEW[state];
}

/** Not yet finished: the engine may still change this agent. */
export function isLive(state: InstanceState): boolean {
	return !TERMINAL_AGENT_STATES.includes(state);
}

/** The person may ask the engine to stop this agent. */
export function canCancel(state: InstanceState): boolean {
	return isLive(state) && state !== 'cancelling';
}

export function agentDurationMs(agent: AgentInfo, now: number): number {
	return Math.max(0, (agent.endedAt ?? now) - agent.spawnedAt);
}

export { formatDuration } from '../conversation/format';

const TIME = new Intl.DateTimeFormat('fr-FR', {
	hour: '2-digit',
	minute: '2-digit',
	second: '2-digit'
});

export function formatTime(ms: number): string {
	return TIME.format(new Date(ms));
}

const COUNT = new Intl.NumberFormat('fr-FR');
const USD = new Intl.NumberFormat('fr-FR', {
	style: 'currency',
	currency: 'USD',
	maximumFractionDigits: 4
});

export function formatUsd(amount: number): string {
	return USD.format(amount);
}

export function formatCount(n: number): string {
	return COUNT.format(n);
}

/** `12 jetons` / `1 jeton`: French uses the singular for 0 and 1. */
export function plural(n: number, one: string, many: string): string {
	return `${formatCount(n)} ${n > 1 ? many : one}`;
}

/** Heading of a successful engine answer to a control action. */
export function controlDoneLabel(action: string): string {
	switch (action) {
		case 'cancel':
			return 'Arrêt demandé.';
		case 'apply_changes':
			return 'Modifications appliquées.';
		case 'discard_changes':
			return 'Modifications abandonnées.';
		default:
			return 'Action effectuée.';
	}
}

/** Heading of a failed engine answer; the engine's own text follows it. */
export function controlFailedLabel(action: string): string {
	switch (action) {
		case 'cancel':
			return 'Arrêt impossible';
		case 'apply_changes':
			return 'Application impossible';
		case 'discard_changes':
			return 'Abandon impossible';
		default:
			return 'Action impossible';
	}
}

export function isolationLabel(isolation: string): string {
	return isolation === 'worktree' ? 'worktree isolé' : 'workspace partagé';
}

export function terminationLabel(t: Termination | undefined): string | null {
	if (!t) return null;
	switch (t.kind) {
		case 'completed':
			return 'Terminé normalement';
		case 'max_turns':
			return `Limite de tours atteinte (${String(t.limit)})`;
		case 'output_truncated':
			return `Sortie tronquée (${String(t.continuations)} continuation(s))`;
		case 'no_progress':
			return `Aucun progrès (${String(t.repeats)} répétitions)`;
		default:
			return String(t.kind);
	}
}

/** Git porcelain letter of a changed file. */
export function changeStatusLabel(code: string): string {
	switch (code) {
		case 'A':
			return 'Ajouté';
		case 'M':
			return 'Modifié';
		case 'D':
			return 'Supprimé';
		case 'R':
			return 'Renommé';
		case 'C':
			return 'Copié';
		default:
			return code;
	}
}

export function changesetLabel(state: ChangeSetState): string {
	switch (state) {
		case 'ready':
			return 'Prêt à appliquer';
		case 'applied':
			return 'Appliqué';
		case 'conflict':
			return 'Conflit';
		case 'discarded':
			return 'Abandonné';
	}
}

export function changesetVariant(state: ChangeSetState): BadgeVariant {
	switch (state) {
		case 'ready':
			return 'default';
		case 'applied':
			return 'secondary';
		case 'conflict':
			return 'destructive';
		case 'discarded':
			return 'outline';
	}
}
