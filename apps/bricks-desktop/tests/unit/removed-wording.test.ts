// Wordings removed from the product (context, session configuration, background jobs, the memory
// settings): none of them is left anywhere under src/, in a view or in a state module. A source
// scan, not a render. Same approach as removed-texts.test.ts.
import { readFileSync, readdirSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const SRC = fileURLToPath(new URL('../../src/', import.meta.url));

/** Every text file of src/, as its path under src/ and its text. */
function sourceFiles(dir: string = SRC): { path: string; text: string }[] {
	return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
		const full = join(dir, entry.name);
		if (entry.isDirectory()) return sourceFiles(full);
		if (!/\.(svelte|ts|js|css|json)$/.test(entry.name)) return [];
		return [{ path: relative(SRC, full), text: readFileSync(full, 'utf8') }];
	});
}

const REMOVED = [
	// Context card: the partial subtotal and the estimate note.
	'Sous-total partiel',
	'pas de tarif pour',
	'Estimation d’après les tarifs configurés',
	// Compaction and clear: the sentence about a run in progress.
	'Une exécution ou une opération est en cours',
	'compaction et vidage attendent sa fin',
	// Memory settings: the note on the former store.
	'L’ancienne mémoire (memory.grafeo)',
	// Session configuration: the duplicated effort lines and the configuration rows.
	'Effort demandé',
	'Effort appliqué',
	'par défaut du modèle',
	'Raisonnement par défaut (configuration)',
	'Jetons de sortie maximum (configuration)',
	'Budget de contexte',
	'Réserve pour la sortie'
];

describe('the removed wordings are gone from src/', () => {
	const files = sourceFiles();

	it('the scan reads the product sources, not an empty folder', () => {
		expect(files.length).toBeGreaterThan(100);
		expect(files.some((f) => f.path === join('lib', 'features', 'session-info', 'ConfigView.svelte'))).toBe(true);
	});

	it.each(REMOVED)('no file under src/ contains « %s »', (text) => {
		expect(files.filter((f) => f.text.includes(text)).map((f) => f.path)).toEqual([]);
	});
});
