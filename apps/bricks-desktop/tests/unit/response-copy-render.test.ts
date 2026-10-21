import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import TimelineEntry from '../../src/lib/features/conversation/timeline/TimelineEntry.svelte';
import { Conversation } from '../../src/lib/state/conversation.svelte';
import { EnvelopeFeed, PROJECT_ID, openedSession } from './fixtures';

function copyButtons(conversation: Conversation): number {
  return conversation.items.reduce((count, item) => {
    const html = render(TimelineEntry, { props: { item, conversation } }).body;
    return count + (html.match(/aria-label="Copier"/g)?.length ?? 0);
  }, 0);
}

describe('one copy action per response in the timeline', () => {
  it('stays absent between tool rounds, appears at run end, and keeps earlier responses', () => {
    const conversation = new Conversation(openedSession(), PROJECT_ID);
    const feed = new EnvelopeFeed({ runId: 'run-copy' });
    conversation.apply(feed.next({ type: 'run_started', prompt: 'Teste', attachments: [], model: 'm' }));
    conversation.apply(feed.next({ type: 'text_delta', text: 'Je vérifie.' }));
    expect(copyButtons(conversation)).toBe(0);
    for (let i = 0; i < 3; i++) {
      conversation.apply(feed.next({ type: 'tool_started', tool_call_id: `tool-${i}`, name: 'Bash', input: {} }));
      conversation.apply(feed.next({ type: 'tool_finished', tool_call_id: `tool-${i}`, name: 'Bash', is_error: false, duration_ms: 1, output: 'ok', executed: true, status: 'success' }));
      expect(copyButtons(conversation)).toBe(0);
      conversation.apply(feed.next({ type: 'text_delta', text: i === 2 ? 'Résultat final.' : 'Je continue.' }));
    }
    expect(copyButtons(conversation)).toBe(0);
    conversation.apply(feed.next({ type: 'run_finished', outcome: 'succeeded', text: 'Résultat final.', turns: 4 }));
    expect(copyButtons(conversation)).toBe(1);
    conversation.apply(feed.next({ type: 'run_started', prompt: 'Encore', attachments: [], model: 'm' }, { runId: 'next-run' }));
    expect(copyButtons(conversation)).toBe(1);
  });

  it('offers one copy per exchange when reopening history without run ids', () => {
    const conversation = new Conversation(openedSession({ history: [
      { role: 'user', attachments: [], blocks: [{ type: 'text', text: 'Teste' }] },
      { role: 'assistant', attachments: [], blocks: [
        { type: 'text', text: 'Je vérifie.' },
        { type: 'tool_use', id: 't1', name: 'Bash', input: {} }
      ] },
      { role: 'assistant', attachments: [], blocks: [{ type: 'text', text: 'Résultat final.' }] },
      { role: 'user', attachments: [], blocks: [{ type: 'text', text: 'Merci' }] },
      { role: 'assistant', attachments: [], blocks: [{ type: 'text', text: 'Avec plaisir.' }] }
    ] }), PROJECT_ID);
    expect(copyButtons(conversation)).toBe(2);
  });
});
