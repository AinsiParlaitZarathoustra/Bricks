// Unit tests for the tool approval options of the settings dialog.
import { describe, expect, it } from 'vitest';
import { TOOL_APPROVAL_OPTIONS, isToolApproval } from '../../src/lib/features/settings/format';

describe('tool approval options', () => {
	it('offers the three policies, in order, with a French label each', () => {
		expect(TOOL_APPROVAL_OPTIONS.map((o) => o.value)).toEqual(['standard', 'always_ask', 'no_confirmation']);
		expect(TOOL_APPROVAL_OPTIONS.map((o) => o.label)).toEqual([
			'Standard',
			'Toujours demander',
			'Sans confirmation'
		]);
	});

	it('explains each policy in one non-empty line', () => {
		for (const option of TOOL_APPROVAL_OPTIONS) {
			expect(option.effect.length).toBeGreaterThan(0);
			expect(option.effect).not.toContain('\n');
		}
	});

	it('accepts only the known policies as select values', () => {
		expect(isToolApproval('always_ask')).toBe(true);
		expect(isToolApproval('no_confirmation')).toBe(true);
		expect(isToolApproval('yolo')).toBe(false);
		expect(isToolApproval(undefined)).toBe(false);
	});
});
