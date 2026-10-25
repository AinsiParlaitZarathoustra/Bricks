// The permission policy as the interface shows and edits it (French labels,
// pure functions). The engine decides; these helpers only build the policy
// a person chose.

import type {
	CustomRules,
	PermissionEffect,
	PermissionProfile,
	SessionPolicy
} from '$lib/ipc/engine';

export const PROFILE_OPTIONS: ReadonlyArray<{ value: PermissionProfile; label: string; detail: string }> = [
	{ value: 'ask', label: 'ASK', detail: 'Lecture libre, le reste demandé' },
	{ value: 'execute', label: 'EXECUTE', detail: 'Tout autorisé' },
	{ value: 'custom', label: 'CUSTOM', detail: 'Règles par action et par outil' }
];

export const EFFECT_OPTIONS: ReadonlyArray<{ value: PermissionEffect; label: string }> = [
	{ value: 'allow', label: 'Autoriser' },
	{ value: 'ask', label: 'Demander' },
	{ value: 'deny', label: 'Refuser' }
];

/** The classes of action of the CUSTOM profile, in the order shown. */
export const CLASS_ROWS: ReadonlyArray<{ key: Exclude<keyof CustomRules, 'tools'>; label: string }> = [
	{ key: 'read', label: 'Lecture et recherche' },
	{ key: 'write', label: 'Écriture' },
	{ key: 'execute', label: 'Commandes et sous-agents' },
	{ key: 'network', label: 'Réseau' }
];

/** The CUSTOM rules a policy starts from: the ASK profile's effects. */
export function defaultCustomRules(): CustomRules {
	return { read: 'allow', write: 'ask', execute: 'ask', network: 'ask', tools: {} };
}

/** Nothing chosen yet: behaves as ASK. */
export function unchosenPolicy(): SessionPolicy {
	return { profile: 'ask', custom: defaultCustomRules(), chosen: false };
}

export function isPermissionProfile(value: unknown): value is PermissionProfile {
	return PROFILE_OPTIONS.some((o) => o.value === value);
}

export function isPermissionEffect(value: unknown): value is PermissionEffect {
	return EFFECT_OPTIONS.some((o) => o.value === value);
}

export function profileLabel(policy: SessionPolicy | null | undefined): string {
	if (!policy || !policy.chosen) return 'Non choisi';
	return PROFILE_OPTIONS.find((o) => o.value === policy.profile)?.label ?? policy.profile;
}

export function effectLabel(effect: PermissionEffect): string {
	return EFFECT_OPTIONS.find((o) => o.value === effect)?.label ?? effect;
}

/** The policy with `profile` chosen; the CUSTOM rules are kept for a later return. */
export function withProfile(policy: SessionPolicy, profile: PermissionProfile): SessionPolicy {
	return { profile, custom: cloneRules(policy.custom), chosen: true };
}

/** The CUSTOM policy with one class of action set to `effect`. */
export function withClass(
	policy: SessionPolicy,
	key: (typeof CLASS_ROWS)[number]['key'],
	effect: PermissionEffect
): SessionPolicy {
	return { profile: 'custom', custom: { ...cloneRules(policy.custom), [key]: effect }, chosen: true };
}

/** The CUSTOM policy with a rule of its own for `tool` (`null` removes it). */
export function withTool(policy: SessionPolicy, tool: string, effect: PermissionEffect | null): SessionPolicy {
	const custom = cloneRules(policy.custom);
	const name = tool.trim();
	if (!name) return policy;
	if (effect === null) delete custom.tools[name];
	else custom.tools[name] = effect;
	return { profile: 'custom', custom, chosen: true };
}

/** The tool rules of a policy, sorted by name. */
export function toolRules(policy: SessionPolicy): Array<{ name: string; effect: PermissionEffect }> {
	return Object.entries(policy.custom.tools)
		.map(([name, effect]) => ({ name, effect }))
		.sort((a, b) => a.name.localeCompare(b.name, 'fr'));
}

function cloneRules(rules: CustomRules): CustomRules {
	return { ...rules, tools: { ...rules.tools } };
}
