// Labels and pure helpers of the catalog views (French, no state).

import { t, type MessageKey, numberFormat } from '$lib/i18n/i18n.svelte';
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
			return t('common.project');
		case 'user':
			return t('catalog.user');
		case 'built_in':
		case 'builtin':
			return t('common.builtIn');
		default:
			return scope;
	}
}

const SKILL_STATES: Record<string, { label: string; variant: BadgeVariant }> = {
	available: { get label() { return t('common.available') }, variant: 'secondary' },
	active: { get label() { return t('common.active') }, variant: 'default' },
	shadowed: { get label() { return t('common.hidden') }, variant: 'outline' },
	invalid: { get label() { return t('common.invalid') }, variant: 'destructive' },
	incompatible: { get label() { return t('common.incompatible') }, variant: 'destructive' },
	pending_approval: { get label() { return t('catalog.awaitingApproval') }, variant: 'outline' },
	refused: { get label() { return t('common.refused') }, variant: 'destructive' },
	disabled: { get label() { return t('common.disabled') }, variant: 'outline' }
};

/** Label and badge of a skill's `state` (see `SkillListing` in the engine). */
export function skillStateView(state: string): { label: string; variant: BadgeVariant } {
	return SKILL_STATES[state] ?? { label: state, variant: 'outline' };
}

/** Status dot and label of a background job's `state`. */
export function jobStatus(state: string): { dot: StatusState; label: string } {
	switch (state) {
		case 'running':
			return { dot: 'running', label: t('common.running') };
		case 'completed':
			return { dot: 'success', label: t('catalog.finished') };
		case 'failed':
			return { dot: 'error', label: t('common.failed') };
		case 'stopped':
			return { dot: 'cancelled', label: t('catalog.stopped') };
		default:
			return { dot: 'idle', label: state };
	}
}

/** The jobs in one line, by state in a fixed order. */
export function jobsSummary(jobs: readonly Pick<JobInfo, 'state'>[]): string {
	if (jobs.length === 0) return t('catalog.noJobs');
	const counts = new Map<string, number>();
	for (const job of jobs) counts.set(job.state, (counts.get(job.state) ?? 0) + 1);
	const words: [string, MessageKey][] = [
		['running', 'catalog.jobsRunning'],
		['completed', 'catalog.jobsFinished'],
		['failed', 'catalog.jobsFailed'],
		['stopped', 'catalog.jobsStopped']
	];
	const parts = words.flatMap(([state, key]) => {
		const count = counts.get(state) ?? 0;
		counts.delete(state);
		return count > 0 ? [t(key, { count })] : [];
	});
	for (const [state, n] of counts) parts.push(`${n} ${state}`);
	return parts.join(' · ');
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

const BYTES = numberFormat({ maximumFractionDigits: 1 });

export function formatBytes(n: number): string {
	if (n < 1024) return t('catalog.b', { n: BYTES.format(n) });
	if (n < 1024 * 1024) return t('catalog.kib', { value: BYTES.format(n / 1024) });
	return t('catalog.mib', { value: BYTES.format(n / (1024 * 1024)) });
}

/** Splits an inspected patch from the engine's cut marker. */
export function splitShortened(text: string): { patch: string; truncated: boolean } {
	return text.endsWith(SHORTENED)
		? { patch: text.slice(0, -SHORTENED.length), truncated: true }
		: { patch: text, truncated: false };
}
