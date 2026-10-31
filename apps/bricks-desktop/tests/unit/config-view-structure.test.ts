// Static checks of the « Configuration » view (sources read, not rendered): the model card has one
// « Effort » line, read from the effective effort (or « aucun »), and no longer the requested and
// applied efforts nor the default-of-the-model wording. The other settings come from the engine's
// rows, and the removed labels are gone. Same approach as context-card-structure.test.ts.
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const src = (path: string) => readFileSync(new URL(`../../src/${path}`, import.meta.url), 'utf8');

const CONFIG = 'lib/features/session-info/ConfigView.svelte';

describe('the model card: one effort line', () => {
	const config = src(CONFIG);

	it('shows one « Effort » line from the effective effort, or « aucun » when there is none', () => {
		expect(config).toContain("{@render field('Effort', config.effective_reasoning ?? 'aucun')}");
	});

	it('no longer has « Effort demandé », « Effort appliqué » nor « par défaut du modèle »', () => {
		expect(config).not.toContain('Effort demandé');
		expect(config).not.toContain('Effort appliqué');
		expect(config).not.toContain('par défaut du modèle');
	});
});

describe('the other settings', () => {
	const config = src(CONFIG);

	it('come from the engine rows, one label and one value each', () => {
		expect(config).toContain('{#each config.settings as entry, i (i)}');
		expect(config).toContain('<dt class="text-muted-foreground">{entry.label}</dt>');
		expect(config).toContain('<dd class="min-w-0 break-words">{entry.value}</dd>');
	});

	it('the permissions card keeps the approval line and the policy editor, and no default-permissions line', () => {
		expect(config).toContain("{@render field('Approbations', config.interactive ? 'demandées' : 'refusées')}");
		expect(config).toContain('<PolicyEditor {policy} tools={toolNames} disabled={saving} onchange={change} />');
		expect(config).not.toContain("field('Permissions'");
	});
});
