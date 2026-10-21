// Engine skill activations in a reopened history: the `<skill_activation id="…">` text the engine
// stores as a user message is not a user message of the timeline. Other user text stays visible.
// The live path (run_started.prompt) never reads that text.
import { describe, expect, it } from 'vitest';
import type { HistoryMessage, RunLabel } from '../../src/lib/ipc/client';
import { Conversation } from '../../src/lib/state/conversation.svelte';
import { EnvelopeFeed, MODEL, PROJECT_ID, T0, openedSession } from './fixtures';

/** The shape of `render_activation` (crates/bricks-skills/src/service.rs). */
const ACTIVATION = `<skill_activation id="act_1" name="wrangler" source="user" revision="r1">
Skill \`wrangler\` is active. Its instructions below are a procedure written by the skill's author.
--- instructions ---
Use the wrangler CLI for Workers.
--- end of instructions ---
</skill_activation>`;

const user = (...texts: string[]): HistoryMessage => ({
	role: 'user',
	attachments: [],
	blocks: texts.map((text) => ({ type: 'text', text }))
});
const assistant = (text: string): HistoryMessage => ({
	role: 'assistant',
	attachments: [],
	blocks: [{ type: 'text', text }]
});

function reopen(history: HistoryMessage[], runLabels: RunLabel[] = []) {
	return new Conversation(openedSession({ history, run_labels: runLabels }), PROJECT_ID);
}

function userTexts(conv: Conversation): string[] {
	return conv.items.flatMap((item) => (item.kind === 'user' ? [item.text] : []));
}

describe('reopened history: engine skill activations', () => {
	it('an activation block of a user message is not shown; the request of that message is', () => {
		const conv = reopen([user(ACTIVATION, 'Analyse ce dossier')]);
		expect(userTexts(conv)).toEqual(['Analyse ce dossier']);
	});

	it('the match ignores the whitespace around the block', () => {
		const conv = reopen([user(`\n  ${ACTIVATION}\n\n`, 'Bonjour')]);
		expect(userTexts(conv)).toEqual(['Bonjour']);
	});

	it('every activation block is skipped, and the messages after them are kept', () => {
		const second = ACTIVATION.replace('act_1', 'act_2').replace('wrangler', 'pdf');
		const conv = reopen([user(ACTIVATION, second), user('Bonjour'), assistant('ok')]);
		expect(userTexts(conv)).toEqual(['Bonjour']);
		expect(conv.items.filter((i) => i.kind === 'assistant')).toHaveLength(1);
	});

	it('other user text is kept, including text that only mentions the tag', () => {
		const mentions = 'Explique la balise <skill_activation id="x"> à un collègue';
		const pastedOpenOnly = '<skill_activation id="x">\ncollé à la main, sans fermeture';
		const otherAttribute = '<skill_activation name="x">\nautre forme</skill_activation>';
		const conv = reopen([user(mentions), user(pastedOpenOnly), user(otherAttribute)]);
		expect(userTexts(conv)).toEqual([mentions, pastedOpenOnly, otherAttribute]);
	});

	it('assistant text is never hidden, even when it looks like an activation', () => {
		const conv = reopen([assistant(ACTIVATION)]);
		const assistants = conv.items.flatMap((item) => (item.kind === 'assistant' ? [item.text] : []));
		expect(assistants).toEqual([ACTIVATION]);
		expect(userTexts(conv)).toEqual([]);
	});

	it('the command label still lands on the request, not on the activation', () => {
		const conv = reopen(
			[user(ACTIVATION), user('Analyse')],
			[{ run_id: 'r1', label: '/skill wrangler', prompt: 'Analyse', at: T0, user_index: null }]
		);
		const users = conv.items.flatMap((item) => (item.kind === 'user' ? [item] : []));
		expect(users.map((u) => [u.text, u.commandLabel])).toEqual([['Analyse', '/skill wrangler']]);
	});
});

describe('live path: a skill activation never becomes a user message', () => {
	it('a run that activates a skill shows its request once, from run_started', () => {
		const conv = new Conversation(openedSession(), PROJECT_ID);
		const feed = new EnvelopeFeed({ runId: 'run-a' });
		conv.apply(feed.next({ type: 'run_started', prompt: 'Analyse', attachments: [], model: MODEL }));
		conv.apply(
			feed.next({
				type: 'skill_activated',
				name: 'wrangler',
				source: 'user',
				revision: 'r1',
				origin: 'user'
			})
		);
		expect(userTexts(conv)).toEqual(['Analyse']);
	});
});
