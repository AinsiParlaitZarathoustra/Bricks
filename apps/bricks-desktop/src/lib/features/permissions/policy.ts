// The permission policy as the interface shows and edits it (French labels,
// pure functions). The engine decides; these helpers only build the policy
// a person chose.

import { t, localeTag } from '$lib/i18n/i18n.svelte'
import type {
	CustomRules,
	PermissionEffect,
	PermissionProfile,
	SessionPolicy
} from '$lib/ipc/engine';

export const PROFILE_OPTIONS: ReadonlyArray<{ value: PermissionProfile; label: string; detail: string }> = [
	{ value: 'ask', label: 'ASK', get detail() { return t('permissions.readsAllowedTheRestAsked') } },
	{ value: 'execute', label: 'EXECUTE', get detail() { return t('permissions.everythingAllowed') } },
	{ value: 'custom', label: 'CUSTOM', get detail() { return t('permissions.rulesPerActionAndPerTool') } }
];

export const EFFECT_OPTIONS: ReadonlyArray<{ value: PermissionEffect; label: string }> = [
	{ value: 'allow', get label() { return t('common.allow') } },
	{ value: 'ask', get label() { return t('permissions.ask') } },
	{ value: 'deny', get label() { return t('common.refuse') } }
];

/** The classes of action of the CUSTOM profile, in the order shown. */
export const CLASS_ROWS: ReadonlyArray<{ key: Exclude<keyof CustomRules, 'tools'>; label: string }> = [
	{ key: 'read', get label() { return t('permissions.readAndSearch') } },
	{ key: 'write', get label() { return t('common.write') } },
	{ key: 'execute', get label() { return t('permissions.commandsAndSubAgents') } },
	{ key: 'network', get label() { return t('permissions.network') } }
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
	if (!policy || !policy.chosen) return t('permissions.notChosen');
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
		.sort((a, b) => a.name.localeCompare(b.name, localeTag()));
}

function cloneRules(rules: CustomRules): CustomRules {
	return { ...rules, tools: { ...rules.tools } };
}
