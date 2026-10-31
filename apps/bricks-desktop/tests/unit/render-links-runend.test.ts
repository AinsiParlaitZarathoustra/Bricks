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
import type { ApprovalRequest, Termination } from '../../src/lib/ipc/engine';

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
	finish: {
		outcome: 'succeeded' | 'failed' | 'cancelled' | 'incomplete';
		error?: string;
		at: number;
		termination?: Termination;
		approvals?: ApprovalRequest[];
	},
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
				termination: finish.termination,
				approvals_unsatisfied: finish.approvals ?? [],
			},
			{ at: finish.at },
		),
	);
	const item = conv.items.find((i): i is RunEndItem => i.kind === 'run_end');
	if (!item) throw new Error('the run has no end item');
	return { conv, item };
}

/** The markup of the card. */
function markup(conv: Conversation, item: RunEndItem): string {
	return render(RunEndCard, { props: { item, conversation: conv } }).body;
}

/** The card's visible text: the spoken totals of DiffStat (sr-only) are left out. */
function visibleCard(conv: Conversation, item: RunEndItem): string {
	return textOf(markup(conv, item).replace(/<span class="sr-only">[^]*?<\/span>/g, ''), ' ');
}

const APPROVAL: ApprovalRequest = {
	approval_id: 'ap-1',
	tool_call_id: 'tc-9',
	tool: 'Bash',
	level: 'execute',
	description: 'Lancer les tests',
	input: { command: 'npm test' },
	preview: null,
};

describe('run-end card: a finished run shows its reason, its error and the files it changed', () => {
	it('renders nothing for a run that edited no file and gave no reason: its state and time are in the work bar', () => {
		for (const outcome of ['succeeded', 'cancelled'] as const) {
			const { conv, item } = runOf({ outcome, at: T0 + 762 }, []);
			expect(visibleCard(conv, item)).toBe('');
		}
	});

	it('lists the edited files, open by default, with their counts and folder, and no time, model or turn count', () => {
		const { conv, item } = runOf({ outcome: 'succeeded', at: T0 + 762 }, [
			{ path: 'src/calcul.ts', kind: 'modify', added: 120, removed: 24 },
		]);
		const text = visibleCard(conv, item);
		expect(text).toContain('1 fichier modifié +120 −24');
		expect(text).toContain('calcul.ts src/');
		expect(text).not.toMatch(/A travaillé|762ms|Terminé|Durée/);
		expect(text).not.toContain(MODEL);
		expect(text).not.toMatch(/\bturns?\b|\btours?\b/i);
		expect(markup(conv, item)).toContain('aria-expanded="true"');
	});

	it('gives each file a button to review its changes and one to show it in the Finder, and no undo', () => {
		const { conv, item } = runOf({ outcome: 'succeeded', at: T0 + 762 }, [
			{ path: 'src/calcul.ts', kind: 'modify', added: 120, removed: 24 },
		]);
		const html = markup(conv, item);
		expect(html).toContain('aria-label="Voir les modifications de calcul.ts"');
		expect(html).toContain('aria-label="Afficher calcul.ts dans le Finder"');
		expect(html).not.toMatch(/Annuler|Undo|Rétablir/i);
	});

	it('never shows a zero beside a real count, in the rows or in the total', () => {
		const { conv, item } = runOf({ outcome: 'succeeded', at: T0 + 762 }, [
			{ path: 'new.ts', kind: 'create', added: 12, removed: 0 },
			{ path: 'old.ts', kind: 'delete', added: 0, removed: 3 },
		]);
		const text = visibleCard(conv, item);
		expect(text).toContain('2 fichiers modifiés +12 −3');
		expect(text).not.toMatch(/−0|\+0\b/);
	});

	it('sums the text edits in the files group, and a binary file adds no count', () => {
		const { conv, item } = runOf({ outcome: 'succeeded', at: T0 + 900 }, [
			{ path: 'a.ts', kind: 'modify', added: 2, removed: 1 },
			{ path: 'img.png', kind: 'create', added: 0, removed: 0, binary: true },
		]);
		const text = visibleCard(conv, item);
		expect(text).toContain('2 fichiers modifiés +2 −1');
		expect(text).toContain('img.png binaire');
	});

	it('shows the error of a failed run as its only line: the state is in the work bar', () => {
		const { conv, item } = runOf({ outcome: 'failed', error: 'quota dépassé', at: T0 + 120 }, []);
		expect(visibleCard(conv, item)).toBe('Erreur : quota dépassé');
	});

	it('gives the reason of an incomplete run and the approvals it never got', () => {
		const { conv, item } = runOf(
			{ outcome: 'incomplete', at: T0 + 120, termination: { kind: 'no_progress', repeats: 3 }, approvals: [APPROVAL] },
			[],
		);
		expect(visibleCard(conv, item)).toBe('Motif : aucun progrès détecté (3 répétitions). 1 autorisation non accordée');
	});
});
