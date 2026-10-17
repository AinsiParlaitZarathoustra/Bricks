// Pure helpers of the « Skills » settings: paging, and the French labels of the sources and states
// the engine reports. No state, no IPC: the calls live in SkillsSettings.svelte.

/** Page size of the engine's skills listing. */
export const SKILLS_PER_PAGE = 20;

export function pageCount(total: number): number {
	return Math.max(1, Math.ceil(total / SKILLS_PER_PAGE));
}

/** A page inside the listing for `total` candidates (the first one when the listing is empty). */
export function clampPage(page: number, total: number): number {
	return Math.min(Math.max(0, page), pageCount(total) - 1);
}

/** « 1 skill », « 3 skills ». */
export function skillCountLabel(count: number): string {
	return `${count} ${count === 1 ? 'skill' : 'skills'}`;
}

const STATE_LABELS: Record<string, string> = {
	available: 'Disponible',
	active: 'Active',
	shadowed: 'Masquée',
	invalid: 'Invalide',
	incompatible: 'Incompatible',
	pending_approval: 'Approbation requise',
	refused: 'Refusée',
	disabled: 'Désactivée'
};

/** The French label of an engine state; an unknown state is shown as the engine wrote it. */
export function skillStateLabel(state: string): string {
	return STATE_LABELS[state] ?? state;
}

export type SkillTone = 'ok' | 'warn' | 'muted';

/** How prominent a state is: usable, needs attention, or simply not in use. */
export function skillStateTone(state: string): SkillTone {
	switch (state) {
		case 'available':
		case 'active':
			return 'ok';
		case 'pending_approval':
		case 'refused':
		case 'invalid':
		case 'incompatible':
			return 'warn';
		default:
			return 'muted';
	}
}

/**
 * The French name of a source. The engine writes `builtin`, `user:<folder>`, `project:<folder>` and
 * `sdk:<id>`; the catalogue's source list uses `builtin`, `user` and `project`.
 */
export function skillSourceLabel(source: string): string {
	if (source === 'builtin') return 'Intégrée';
	if (source === 'user') return 'Personnelle';
	if (source === 'project') return 'Projet';
	if (source.startsWith('user:')) return `Personnelle · ${source.slice('user:'.length)}`;
	if (source.startsWith('project:')) return `Projet · ${source.slice('project:'.length)}`;
	if (source.startsWith('sdk:')) return `SDK · ${source.slice('sdk:'.length)}`;
	return source;
}
