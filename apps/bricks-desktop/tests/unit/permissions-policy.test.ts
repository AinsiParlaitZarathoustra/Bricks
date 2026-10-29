// The permission policy helpers the settings and the policy editor use (pure functions, no
// engine): the profile and class labels, the unchosen policy, and the edits that keep the
// CUSTOM rules of a policy and never change the policy they are given.
import { describe, expect, it } from 'vitest';
import {
	CLASS_ROWS,
	EFFECT_OPTIONS,
	PROFILE_OPTIONS,
	defaultCustomRules,
	effectLabel,
	isPermissionEffect,
	isPermissionProfile,
	profileLabel,
	toolRules,
	unchosenPolicy,
	withClass,
	withProfile,
	withTool
} from '../../src/lib/features/permissions/policy';
import type { SessionPolicy } from '../../src/lib/ipc/engine';

const custom: SessionPolicy = {
	profile: 'custom',
	custom: { read: 'allow', write: 'deny', execute: 'ask', network: 'deny', tools: { Bash: 'allow' } },
	chosen: true
};

describe('labels and options', () => {
	it('offers the three profiles in order, named as the engine names them', () => {
		expect(PROFILE_OPTIONS.map((o) => o.value)).toEqual(['ask', 'execute', 'custom']);
		expect(PROFILE_OPTIONS.map((o) => o.label)).toEqual(['ASK', 'EXECUTE', 'CUSTOM']);
	});

	it('offers the three effects, in French', () => {
		expect(EFFECT_OPTIONS.map((o) => o.value)).toEqual(['allow', 'ask', 'deny']);
		expect(EFFECT_OPTIONS.map((o) => o.label)).toEqual(['Autoriser', 'Demander', 'Refuser']);
	});

	it('edits the four classes of action of the CUSTOM profile', () => {
		expect(CLASS_ROWS.map((r) => r.key)).toEqual(['read', 'write', 'execute', 'network']);
	});

	it('labels a policy by its profile, or as not chosen', () => {
		expect(profileLabel(unchosenPolicy())).toBe('Non choisi');
		expect(profileLabel(null)).toBe('Non choisi');
		expect(profileLabel({ ...custom, profile: 'execute', chosen: true })).toBe('EXECUTE');
		expect(profileLabel(custom)).toBe('CUSTOM');
		expect(effectLabel('allow')).toBe('Autoriser');
		expect(effectLabel('deny')).toBe('Refuser');
	});

	it('accepts only known profiles and effects', () => {
		expect(isPermissionProfile('custom')).toBe(true);
		expect(isPermissionProfile('yolo')).toBe(false);
		expect(isPermissionEffect('deny')).toBe(true);
		expect(isPermissionEffect('standard')).toBe(false);
		expect(isPermissionEffect(undefined)).toBe(false);
	});
});

describe('the policy a session starts from', () => {
	it('is not chosen, and behaves as ASK: reads run, the rest is asked', () => {
		const policy = unchosenPolicy();
		expect(policy.chosen).toBe(false);
		expect(policy.profile).toBe('ask');
		expect(policy.custom).toEqual(defaultCustomRules());
	});

	it('has the ASK effects as its CUSTOM starting point', () => {
		expect(defaultCustomRules()).toEqual({ read: 'allow', write: 'ask', execute: 'ask', network: 'ask', tools: {} });
	});
});

describe('edits return a new policy and keep the rules', () => {
	it('choosing a profile keeps the CUSTOM rules for a later return', () => {
		const executed = withProfile(custom, 'execute');
		expect(executed).toEqual({ profile: 'execute', custom: custom.custom, chosen: true });
		expect(withProfile(executed, 'custom').custom).toEqual(custom.custom);
	});

	it('setting a class makes the policy CUSTOM and changes only that class', () => {
		const changed = withClass(custom, 'network', 'allow');
		expect(changed.profile).toBe('custom');
		expect(changed.chosen).toBe(true);
		expect(changed.custom).toEqual({ ...custom.custom, network: 'allow' });
	});

	it('a tool rule is set, renamed with its trimmed name, and removed with null', () => {
		const set = withTool(custom, ' Edit ', 'deny');
		expect(set.custom.tools).toEqual({ Bash: 'allow', Edit: 'deny' });
		const removed = withTool(set, 'Bash', null);
		expect(removed.custom.tools).toEqual({ Edit: 'deny' });
	});

	it('a blank tool name changes nothing', () => {
		expect(withTool(custom, '   ', 'allow')).toBe(custom);
	});

	it('never changes the policy it is given', () => {
		const before = JSON.stringify(custom);
		withClass(custom, 'write', 'allow');
		withTool(custom, 'Write', 'allow');
		withTool(custom, 'Bash', null);
		withProfile(custom, 'ask');
		expect(JSON.stringify(custom)).toBe(before);
	});
});

describe('tool rules', () => {
	it('are listed by name, in French order', () => {
		const policy = withTool(withTool(custom, 'Write', 'deny'), 'edit', 'ask');
		expect(toolRules(policy)).toEqual([
			{ name: 'Bash', effect: 'allow' },
			{ name: 'edit', effect: 'ask' },
			{ name: 'Write', effect: 'deny' }
		]);
	});

	it('are empty when no tool has its own rule', () => {
		expect(toolRules(unchosenPolicy())).toEqual([]);
	});
});
