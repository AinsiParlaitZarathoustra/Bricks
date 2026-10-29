import { describe, expect, it } from 'vitest';
import { cn } from '../../src/lib/utils';

describe('custom typography class merging', () => {
	it('keeps a text color alongside a custom font size', () => {
		expect(cn('text-ui text-muted-foreground')).toBe('text-ui text-muted-foreground');
	});

	it('merges custom and built-in sizes without dropping text color', () => {
		expect(cn('text-sm text-foreground text-ui')).toBe('text-foreground text-ui');
	});

	it('merges each custom size in the font-size group', () => {
		expect(cn('text-ui text-caption text-code text-compact')).toBe('text-compact');
	});

	it('resolves custom font sizes independently for each variant', () => {
		expect(cn('text-ui hover:text-caption hover:text-code')).toBe('text-ui hover:text-code');
	});
});
