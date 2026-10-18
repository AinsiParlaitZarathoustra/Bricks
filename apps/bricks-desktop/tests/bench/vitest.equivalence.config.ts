// Config of the Sprint 14.5 regression checks (tests/bench): the unit config, with this folder's tests only.
import base from '../vitest.config.ts';

export default {
	...base,
	test: {
		...base.test,
		include: ['tests/bench/**/*.test.ts'],
		testTimeout: 600_000
	}
};
