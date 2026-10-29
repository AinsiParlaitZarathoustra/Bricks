// Static checks of the context card and of the composer ring (sources read, not rendered): the
// labels of the card are the ones the spec keeps, the removed wordings are gone, and the ring
// reads the same figures as the card. Same approach as removed-texts.test.ts.
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const src = (path: string) => readFileSync(new URL(`../../src/${path}`, import.meta.url), 'utf8');

const CARD = 'lib/features/context/ContextCard.svelte';
const RING = 'lib/features/context/ContextGauge.svelte';

describe('context card: compact, one share, one meter, no session block', () => {
	const card = src(CARD);

	it('keeps the title and reads the share and the figures from contextGauge and formatCompact', () => {
		expect(card).toContain('<SectionCard title="Contexte">');
		expect(card).toContain('contextGauge(conversation.context ?? null, conversation.contextStale ?? false)');
		expect(card).toContain('formatPercent(gauge.percent)');
		expect(card).toContain('formatCompact(gauge.used)');
		expect(card).toContain('formatCompact(gauge.total)');
	});

	it('shows one meter bar with its role and its value, and an unknown share as a dash', () => {
		expect(card).toContain('role="meter"');
		expect(card).toContain("aria-label=\"Contexte utilisé\"");
		expect(card).toContain("gauge.percent === null ? '—' : formatPercent(gauge.percent)");
		expect(card).toContain("'inconnu'");
	});

	it('has no approximation sign', () => {
		expect(card).not.toContain('≈');
	});

	it('has no session consumption block and none of its row labels', () => {
		const removed = [
			'Consommation session',
			'Consommation cumulée',
			'Écriture du cache',
			'Raisonnement',
			'non tarifé',
			'Requêtes',
			'Montant partiel',
			'non mesuré',
		];
		expect(removed.filter((t) => card.includes(t))).toEqual([]);
		expect(card).not.toContain('<dt');
		expect(card).not.toContain('contextLine(');
	});
});

describe('composer ring: same figures as the card, accessible name kept', () => {
	const ring = src(RING);

	it('takes its value from contextGauge and gaugeLine, the card from contextLine', () => {
		expect(ring).toContain('contextGauge(conversation?.context ?? null, conversation?.contextStale ?? false)');
		expect(ring).toContain('gaugeLine(gauge)');
	});

	it('names the ring « Contexte d’entrée : inconnu » when there is no share, and gives the cache rate as a dash when unknown', () => {
		expect(ring).toContain("'Contexte d’entrée : inconnu'");
		expect(ring).toContain("`Contexte d’entrée : ${value}`");
		expect(ring).toContain("cache === null ? '—' : formatPercent(cache)");
		expect(ring).toContain('Taux de cache moyen');
	});

	it('keeps the drawn fill bounded through the gauge ratio', () => {
		expect(ring).toContain('gauge.ratio * CIRCUMFERENCE');
		expect(ring).not.toMatch(/gauge\.percent\s*\*\s*CIRCUMFERENCE/);
	});
});
