// When the computer must stay awake (src/lib/state/keepAwake.ts): always while the preference is on,
// otherwise only while a conversation that asked for it runs. Pure function: plain objects stand
// for the conversations.
import { describe, expect, it } from 'vitest';
import { keepAwakeWanted } from '../../src/lib/state/keepAwake';

const conversation = (keepAwake: boolean, running: boolean) => ({ keepAwake, running });

describe('keepAwakeWanted', () => {
	it('is wanted at all times when the preference is on, whatever the conversations', () => {
		expect(keepAwakeWanted(true, [])).toBe(true);
		expect(keepAwakeWanted(true, [conversation(false, false)])).toBe(true);
	});

	it('is not wanted when the preference is off and no conversation asks for it', () => {
		expect(keepAwakeWanted(false, [])).toBe(false);
		expect(keepAwakeWanted(false, [conversation(false, true), conversation(false, false)])).toBe(false);
	});

	it('is wanted while a conversation that asked for it runs', () => {
		expect(keepAwakeWanted(false, [conversation(true, true)])).toBe(true);
		expect(keepAwakeWanted(false, [conversation(false, false), conversation(true, true)])).toBe(true);
	});

	it('an idle conversation that asked for it does not hold the computer awake', () => {
		expect(keepAwakeWanted(false, [conversation(true, false)])).toBe(false);
	});

	it('reads any iterable, such as the values of the conversations map', () => {
		const open = new Map([
			['a', conversation(false, false)],
			['b', conversation(true, true)]
		]);
		expect(keepAwakeWanted(false, open.values())).toBe(true);
		expect(keepAwakeWanted(false, new Map().values())).toBe(false);
	});
});
