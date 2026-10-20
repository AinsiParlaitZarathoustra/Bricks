// Server render of two.3 views: the user message (links in plain text, `dir="auto"`,
// HTML-like text shown as text) and the run-end card (the « bilan » line built from the
// reducer's envelopes, as the engine sends them). Node environment, rendered with svelte/server,
// like toolline-render.test.ts. Validation, Haiku V.
import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import UserMessage from '../../src/lib/features/conversation/timeline/UserMessage.svelte';
import RunEndCard from '../../src/lib/features/conversation/timeline/RunEndCard.svelte';
import { Conversation, type RunEndItem, type UserItem } from '../../src/lib/state/conversation.svelte';
import { EnvelopeFeed, MODEL, PROJECT_ID, T0, openedSession } from './fixtures';

/** Narrow no-break spaces from fr-FR grouping are in the product; the tests compare plain text. */
const plain = (s: string) => s.replace(/[  ]/g, ' ');

/** The markup's visible text, comments removed, entities decoded, spaces collapsed. Tags are replaced by `sep`. */
function textOf(html: string, sep = ''): string {
	const decoded = html
		.replace(/<!--[^]*?-->/g, '')
		.replace(/<[^>]+>/g, sep)
		.replace(/&lt;/g, '<')
		.replace(/&gt;/g, '>')
		.replace(/&quot;/g, '"')
		.replace(/&#39;/g, "'")
		.replace(/&amp;/g, '&');
	return plain(decoded.replace(/\s+/g, ' ').trim());
}

/** The anchors of the markup: decoded href and visible text. */
function anchorsOf(html: string): { href: string; text: string; attrs: string }[] {
	const decode = (s: string) => s.replace(/&amp;/g, '&').replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&quot;/g, '"').replace(/&#39;/g, "'");
	return [...html.matchAll(/<a\b([^>]*)>([\s\S]*?)<\/a>/g)].map((m) => {
		const href = /\bhref="([^"]*)"/.exec(m[1])?.[1] ?? '';
		return { href: decode(href), text: decode(m[2].replace(/<[^>]+>/g, '')), attrs: m[1] };
	});
}

function userMessage(text: string): string {
	const item: UserItem = { kind: 'user', id: 'u1', text, attachments: [] };
	return render(UserMessage, { props: { item } }).body;
}

describe('user message: web addresses become links, the rest stays text', () => {
	it('links an address with a query string and an ampersand, whole, and leaves the final dot out', () => {
		const html = userMessage('Voir https://exemple.fr/recherche?q=a&lang=fr&page=2.');
		expect(anchorsOf(html)).toEqual([
			expect.objectContaining({ href: 'https://exemple.fr/recherche?q=a&lang=fr&page=2', text: 'https://exemple.fr/recherche?q=a&lang=fr&page=2' })
		]);
		expect(textOf(html)).toContain('Voir https://exemple.fr/recherche?q=a&lang=fr&page=2.');
	});

	it('links a www. address with https as the target, and keeps a comma outside the link', () => {
		const html = userMessage('Le site www.exemple.fr/docs, puis la suite');
		const [link] = anchorsOf(html);
		expect(link.href).toBe('https://www.exemple.fr/docs');
		expect(link.text).toBe('www.exemple.fr/docs');
		expect(textOf(html)).toContain('www.exemple.fr/docs, puis la suite');
	});

	it('opens links without navigating: rel is set, the handler is the system opener', () => {
		const html = userMessage('https://exemple.fr');
		const [link] = anchorsOf(html);
		expect(link.attrs).toContain('rel="noopener noreferrer nofollow"');
	});

	it('does not link an address inside a code span or a fenced block', () => {
		expect(anchorsOf(userMessage('Lance `curl https://exemple.fr/api` maintenant'))).toEqual([]);
		expect(anchorsOf(userMessage('```\nhttps://exemple.fr\n```'))).toEqual([]);
	});

	it('shows HTML-like text as text: nothing is executed or rendered as an element', () => {
		const html = userMessage('<img src=x onerror="alert(1)"> <script>alert(2)</script> https://exemple.fr');
		expect(html).not.toMatch(/<img\b/i);
		expect(html).not.toMatch(/<script\b/i);
		expect(html).not.toMatch(/<[^>]*\sonerror=/i);
		expect(textOf(html)).toContain('<img src=x onerror="alert(1)"> <script>alert(2)</script>');
		expect(anchorsOf(html).map((a) => a.href)).toEqual(['https://exemple.fr']);
	});

	it('keeps text unchanged when there is no address, and keeps the bubble direction automatic', () => {
		const html = userMessage('مرحبا بالعالم and plain French: ça va ?');
		expect(html).toContain('dir="auto"');
		expect(html).toContain('text-start');
		expect(textOf(html)).toContain('مرحبا بالعالم and plain French: ça va ?');
		expect(anchorsOf(html)).toEqual([]);
	});
});

// ─── Run-end card, built from the envelopes the engine sends ───────────────

/** A conversation fed with the envelopes of one run, as the engine streams them. */
function runOf(
	finish: { outcome: 'succeeded' | 'failed' | 'cancelled' | 'incomplete'; error?: string; at: number },
	edits: Array<{ path: string; kind: 'create' | 'modify' | 'delete'; added: number; removed: number; binary?: boolean }>,
) {
	const feed = new EnvelopeFeed({ runId: 'run-1' });
	const conv = new Conversation(openedSession(), PROJECT_ID);
	conv.apply(
		feed.next({ type: 'run_started', prompt: 'Corrige le calcul', attachments: [], model: MODEL }, { at: T0 }),
	);
	if (edits.length > 0) {
		conv.apply(
			feed.next(
				{ type: 'edit_applied', tool_call_id: 't-1', tool: 'Edit', files: edits },
				{ at: T0 + 300 },
			),
		);
	}
	conv.apply(
		feed.next(
			{
				type: 'run_finished',
				outcome: finish.outcome,
				error: finish.error,
				text: 'Fait.',
				turns: 3,
			},
			{ at: finish.at },
		),
	);
	const item = conv.items.find((i): i is RunEndItem => i.kind === 'run_end');
	if (!item) throw new Error('the run has no end item');
	return { conv, item };
}

function card(conv: Conversation, item: RunEndItem): string {
	return textOf(render(RunEndCard, { props: { item, conversation: conv } }).body, ' ');
}

/**
 * The header line only: the card text before the reason, the error and the two groups
 * (« N fichiers modifiés », « N commandes exécutées »).
 */
function summaryLine(conv: Conversation, item: RunEndItem): string {
	return card(conv, item)
		.split(/Motif :|Erreur :|\d+ fichiers? modifiés?|\d+ commandes? exécutées?/)[0]
		.trim();
}

/** The files group trigger, with the spoken totals of DiffStat removed. */
function filesGroup(conv: Conversation, item: RunEndItem): string | null {
	const m = card(conv, item).match(/\d+ fichiers? modifiés?[^]*$/);
	if (!m) return null;
	return m[0].replace(/\s*\d+ ajouts?, \d+ suppressions?/, '').split(/\d+ commandes? exécutées?/)[0].trim();
}

describe('run-end card: one compact header and closed groups from the real run data', () => {
	it('reads « Terminé · A travaillé MM:SS » with no turn count, no model id and no « Durée : » prefix', () => {
		const { conv, item } = runOf({ outcome: 'succeeded', at: T0 + 762 }, [
			{ path: 'src/calcul.ts', kind: 'modify', added: 120, removed: 24 },
		]);
		const text = card(conv, item);
		expect(summaryLine(conv, item)).toBe('Terminé · A travaillé 00:00');
		expect(filesGroup(conv, item)).toMatch(/^1 fichier modifié \+120 −24 calcul\.ts/);
		// The group is closed by default.
		expect(render(RunEndCard, { props: { item, conversation: conv } }).body).toContain('aria-expanded="false"');
		expect(text).not.toContain('Durée');
		expect(text).not.toContain(MODEL);
		expect(text).not.toMatch(/\btours?\b/i);
		expect(text).not.toMatch(/\bturns?\b/i);
	});

	it('sums the text edits of the run in the files group, a binary file adding no counts', () => {
		const { conv, item } = runOf({ outcome: 'succeeded', at: T0 + 900 }, [
			{ path: 'a.ts', kind: 'modify', added: 2, removed: 1 },
			{ path: 'img.png', kind: 'create', added: 0, removed: 0, binary: true },
		]);
		expect(summaryLine(conv, item)).toBe('Terminé · A travaillé 00:00');
		expect(filesGroup(conv, item)).toMatch(/^2 fichiers modifiés \+2 −1 a\.ts/);
		expect(card(conv, item)).toContain('img.png binaire');
	});

	it('shows no files group when the run edited nothing', () => {
		const { conv, item } = runOf({ outcome: 'succeeded', at: T0 + 40 }, []);
		expect(summaryLine(conv, item)).toBe('Terminé · A travaillé 00:00');
		expect(filesGroup(conv, item)).toBeNull();
	});

	it('keeps the failed state and its error, and the cancelled state without a second sentence', () => {
		const failed = runOf({ outcome: 'failed', error: 'quota dépassé', at: T0 + 120 }, []);
		const failedText = card(failed.conv, failed.item);
		expect(summaryLine(failed.conv, failed.item)).toBe('Échec · A travaillé 00:00');
		expect(failedText).toContain('Erreur : quota dépassé');

		const cancelled = runOf({ outcome: 'cancelled', at: T0 + 120 }, []);
		const cancelledText = card(cancelled.conv, cancelled.item);
		expect(summaryLine(cancelled.conv, cancelled.item)).toBe('Annulé · A travaillé 00:00');
		expect(cancelledText).not.toMatch(/Erreur|Motif/);
	});
});
