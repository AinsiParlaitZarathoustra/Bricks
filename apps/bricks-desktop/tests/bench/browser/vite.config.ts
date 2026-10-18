// Bundles tests/bench/browser/entry.ts into one classic script for Chromium (see run.sh).
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';

const here = fileURLToPath(new URL('.', import.meta.url));

export default defineConfig({
	root: here,
	logLevel: 'warn',
	build: {
		outDir: fileURLToPath(new URL('../results/browser', import.meta.url)),
		emptyOutDir: true,
		minify: false,
		sourcemap: false,
		lib: {
			entry: fileURLToPath(new URL('./entry.ts', import.meta.url)),
			name: 'Eq',
			formats: ['iife'],
			fileName: () => 'equivalence.js'
		}
	}
});
