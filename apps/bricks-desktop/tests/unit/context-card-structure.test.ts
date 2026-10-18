// Static checks of the context card and of the composer ring (sources read, not rendered): the
// labels of the card are the ones the spec keeps, the removed wordings are gone, and the ring
// reads the same figures as the card. Same approach as removed-texts.test.ts.
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const src = (path: string) => readFileSync(new URL(`../../src/${path}`, import.meta.url), 'utf8');

const CARD = 'lib/features/context/ContextCard.svelte';
const RING = 'lib/features/context/ContextGauge.svelte';

describe('context card: exact labels and no removed wording', () => {
	const card = src(CARD);

	it('keeps the title, the session block and its row labels', () => {
		expect(card).toContain('<SectionCard title="Contexte">');
		expect(card).toContain('Consommation session');
		for (const label of ['Entrée', 'Sortie', 'Cache', 'Requêtes', 'Coût']) {
			expect(card, label).toContain(`>${label}</dt>`);
		}
	});

	it('shows the context line from contextLine, with no approximation sign', () => {
		expect(card).toContain('contextLine(status, stale)');
		expect(card).not.toContain('≈');
	});

	it('does not keep the removed wordings of the card', () => {
		const removed = ['Écriture du cache', 'Raisonnement', 'non tarifé', 'Consommation cumulée'];
		expect(removed.filter((t) => card.includes(t))).toEqual([]);
	});

	it('reads the cost as a measured amount or « non mesuré », and says when it is partial', () => {
		expect(card).toContain('non mesuré');
		expect(card).toContain('Montant partiel : certaines réponses n’ont pas de prix');
	});
});

describe('composer ring: same figures as the card, accessible name kept', () => {
	const ring = src(RING);

	it('takes its value from contextGauge and gaugeLine, the card from contextLine', () => {
		expect(ring).toContain('contextGauge(conversation?.context ?? null, conversation?.contextStale ?? false)');
		expect(ring).toContain('gaugeLine(gauge)');
	});

	it('names the ring « Contexte : inconnu » when there is no share, and gives the cache rate as a dash when unknown', () => {
		expect(ring).toContain("'Contexte : inconnu'");
		expect(ring).toContain("`Contexte : ${value}`");
		expect(ring).toContain("cache === null ? '—' : formatPercent(cache)");
		expect(ring).toContain('Taux de cache moyen');
	});

	it('keeps the drawn fill bounded through the gauge ratio', () => {
		expect(ring).toContain('gauge.ratio * CIRCUMFERENCE');
		expect(ring).not.toMatch(/gauge\.percent\s*\*\s*CIRCUMFERENCE/);
	});
});
