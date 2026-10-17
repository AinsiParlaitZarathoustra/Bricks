// Labels and pure helpers of the catalog views (French, no state).

import type { BadgeVariant } from '$lib/components/ui/badge';
import type { StatusState } from '$lib/layout/StatusDot.svelte';
import type { InspectorState } from '$lib/state/ui.svelte';
import type { JobInfo } from '$lib/state/conversation.svelte';

/** Entries per page of the engine's `list_agent_profiles` and `list_skills` answers. */
export const ENGINE_PAGE_SIZE = 20;

/** Suffix the engine adds to an inspected patch that it had to cut. */
const SHORTENED = '\n[shortened]';

/** Text of an inspector argument, or null when absent or empty. */
export function argText(value: InspectorState['args'][string] | unknown): string | null {
	return typeof value === 'string' && value.trim() ? value.trim() : null;
}

export function pageCount(total: number, perPage: number): number {
	return Math.max(1, Math.ceil(total / perPage));
}

/** `ProfileScope` as the engine serializes it (`built_in` for the bundled profiles). */
export function scopeLabel(scope: string): string {
	switch (scope) {
		case 'project':
			return 'Projet';
		case 'user':
			return 'Utilisateur';
		case 'built_in':
		case 'builtin':
			return 'Intégré';
		default:
			return scope;
	}
}

const SKILL_STATES: Record<string, { label: string; variant: BadgeVariant }> = {
	available: { label: 'Disponible', variant: 'secondary' },
	active: { label: 'Active', variant: 'default' },
	shadowed: { label: 'Masquée', variant: 'outline' },
	invalid: { label: 'Invalide', variant: 'destructive' },
	incompatible: { label: 'Incompatible', variant: 'destructive' },
	pending_approval: { label: 'En attente d’approbation', variant: 'outline' },
	refused: { label: 'Refusée', variant: 'destructive' },
	disabled: { label: 'Désactivée', variant: 'outline' }
};

/** Label and badge of a skill's `state` (see `SkillListing` in the engine). */
export function skillStateView(state: string): { label: string; variant: BadgeVariant } {
	return SKILL_STATES[state] ?? { label: state, variant: 'outline' };
}

/** Status dot and label of a background job's `state`. */
export function jobStatus(state: string): { dot: StatusState; label: string } {
	switch (state) {
		case 'running':
			return { dot: 'running', label: 'En cours' };
		case 'completed':
			return { dot: 'success', label: 'Terminée' };
		case 'failed':
			return { dot: 'error', label: 'Échec' };
		case 'stopped':
			return { dot: 'cancelled', label: 'Arrêtée' };
		default:
			return { dot: 'idle', label: state };
	}
}

/** Running jobs first, then the most recent. */
export function sortJobs(jobs: JobInfo[]): JobInfo[] {
	return [...jobs].sort(
		(a, b) =>
			Number(b.state === 'running') - Number(a.state === 'running') || b.startedAt - a.startedAt
	);
}

/** A job's duration: its end, or the time it has run so far. */
export function jobDurationMs(job: JobInfo, now: number): number {
	return job.durationMs ?? Math.max(0, now - job.startedAt);
}

const BYTES = new Intl.NumberFormat('fr-FR', { maximumFractionDigits: 1 });

export function formatBytes(n: number): string {
	if (n < 1024) return `${BYTES.format(n)} o`;
	if (n < 1024 * 1024) return `${BYTES.format(n / 1024)} Kio`;
	return `${BYTES.format(n / (1024 * 1024))} Mio`;
}

/** Splits an inspected patch from the engine's cut marker. */
export function splitShortened(text: string): { patch: string; truncated: boolean } {
	return text.endsWith(SHORTENED)
		? { patch: text.slice(0, -SHORTENED.length), truncated: true }
		: { patch: text, truncated: false };
}
