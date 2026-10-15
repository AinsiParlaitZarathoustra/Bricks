// Unit tests for the pure frontend logic (reducers, formatters).
// Run from apps/bricks-desktop: npx vitest run --config tests/vitest.config.ts
// Dev dependency to add (not added here): vitest.
import { fileURLToPath } from 'node:url';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { defineConfig } from 'vitest/config';

const appRoot = fileURLToPath(new URL('..', import.meta.url));

export default defineConfig({
	root: appRoot,
	// Compiles `.svelte.ts` modules (runes) such as src/lib/state/conversation.svelte.ts.
	plugins: [svelte()],
	resolve: {
		// Svelte's documented setting for Vitest: use the `browser` entry points of packages.
		conditions: ['browser'],
		alias: { $lib: fileURLToPath(new URL('../src/lib', import.meta.url)) }
	},
	test: {
		environment: 'node',
		include: ['tests/unit/**/*.test.ts']
	}
});
