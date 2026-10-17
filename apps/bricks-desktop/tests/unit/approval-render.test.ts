// Server render of the tool approval card (ApprovalCard.svelte): a pending request shows its
// three decisions; a resolved one is a single trace line with no decision left to take.
//.2, Haiku 4 (validation). Node environment, rendered to a string.
import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import ApprovalCard from '../../src/lib/features/conversation/timeline/ApprovalCard.svelte';
import type { Conversation } from '../../src/lib/state/conversation.svelte';
import type { ApprovalRequest } from '../../src/lib/ipc/engine';

const plain = (s: string) => s.replace(/[\u00a0\u202f]/g, ' ');

/** Plain text of a rendered fragment (tags and comments removed). */
function text(html: string): string {
	return plain(html.replace(/<!--[^]*?-->/g, '').replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ').trim());
}

/** The card reads the conversation only for sub-agents; none here. */
const conversation = { agents: new Map() } as unknown as Conversation;

const approval = {
	approval_id: 'ap-1',
	tool_call_id: 'tc-1',
	tool: 'Write',
	level: 'write',
	description: 'Écrire notes.txt',
	input: { file_path: '/work/demo/notes.txt', content: 'bonjour' },
	preview: null,
	agent_id: null
} as unknown as ApprovalRequest;

describe('ApprovalCard: pending request', () => {
	it('offers the three decisions in French, in one labelled region', () => {
		const { body } = render(ApprovalCard, { props: { approval, conversation } });
		const shown = text(body);
		expect(shown).toContain('Autorisation demandée');
		expect(shown).toContain('Autoriser');
		expect(shown).toContain('Autoriser pour la session');
		expect(shown).toContain('Refuser');
		expect(body).toContain('aria-label="Autorisation demandée"');
	});

	it('shows the tool and its level, and says nothing about the outcome yet', () => {
		const shown = text(render(ApprovalCard, { props: { approval, conversation } }).body);
		expect(shown).toContain('Write');
		expect(shown).toContain('Niveau : write');
		expect(shown).not.toMatch(/Autorisé ·|Refusé ·/);
	});
});

describe('ApprovalCard: resolved request', () => {
	it('is one trace line: decision, tool and who decided', () => {
		const { body } = render(ApprovalCard, {
			props: { approval, resolved: { decision: 'allow', by: 'user' }, conversation }
		});
		expect(text(body)).toBe('Autorisé · Write · par vous');
	});

	it('keeps no decision button and no card region once resolved', () => {
		const { body } = render(ApprovalCard, {
			props: { approval, resolved: { decision: 'deny', by: 'user' }, conversation }
		});
		expect(body).not.toContain('<button');
		expect(body).not.toContain('aria-label="Autorisation demandée"');
		expect(text(body)).toBe('Refusé · Write · par vous');
	});

	it('names the session grant when the decision came from it', () => {
		const { body } = render(ApprovalCard, {
			props: { approval, resolved: { decision: 'allow_for_session', by: 'session' }, conversation }
		});
		expect(text(body)).toBe('Autorisé pour la session · Write · selon l’autorisation de la session');
	});
});
