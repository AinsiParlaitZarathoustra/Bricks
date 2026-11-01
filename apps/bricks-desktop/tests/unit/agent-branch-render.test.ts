// @vitest-environment happy-dom
// The branch of a sub-agent mounted (AgentBranch.svelte): the mission, the tool call and its output,
// the answer, and the fixed bar that says nothing can be written to a sub-agent (no field, no form).
// The user message of a branch offers no revision (UserMessage.svelte), where a conversation does.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import AgentBranch from '../../src/lib/features/agents/AgentBranch.svelte';
import UserMessage from '../../src/lib/features/conversation/timeline/UserMessage.svelte';
import { Conversation, type UserItem } from '../../src/lib/state/conversation.svelte';
import { EnvelopeFeed, PROJECT_ID, T0, agentResult, openedSession, spawnInfo } from './fixtures';

const ipc = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => {
	class Channel {
		onmessage: ((event: unknown) => void) | null = null;
	}
	return { Channel, invoke: (cmd: string, args?: unknown) => ipc.invoke(cmd, args) };
});
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }));
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn() }));

const AGENT = 'ag-1';
const RUN = 'run-1';

/** A parent conversation whose sub-agent has run, read and answered, and finished. */
function finishedParent(): { conv: Conversation } {
	const conv = new Conversation(openedSession(), PROJECT_ID);
	const feed = new EnvelopeFeed({ runId: RUN });
	conv.apply(feed.next({ type: 'run_started', prompt: 'Cherche', attachments: [], model: 'm' }, { at: T0 }));
	conv.apply(feed.next({ type: 'tool_started', tool_call_id: 'tc-A', name: 'Agent', input: {} }));
	conv.apply(feed.next({ type: 'agent_spawned', agent: spawnInfo({ agent_id: AGENT, tool_call_id: 'tc-A', description: 'Lire le code' }) }));
	conv.apply(feed.next({ type: 'agent_tool_started', agent_id: AGENT, tool_call_id: 'tc-1', name: 'Read', input: { file_path: 'a.ts' } }));
	conv.apply(
		feed.next({
			type: 'agent_tool_finished',
			agent_id: AGENT,
			tool_call_id: 'tc-1',
			name: 'Read',
			is_error: false,
			duration_ms: 30,
			executed: true,
			status: 'success',
			output: 'const a = 1;',
			output_truncated: false,
		}),
	);
	conv.apply(feed.next({ type: 'agent_text', agent_id: AGENT, text: 'La constante vaut un.' }));
	conv.apply(feed.next({ type: 'agent_finished', result: agentResult({ agent_id: AGENT }) }));
	return { conv };
}

let target: HTMLDivElement;
let app: ReturnType<typeof mount> | null = null;

beforeEach(() => {
	vi.stubGlobal(
		'ResizeObserver',
		class {
			observe() {}
			unobserve() {}
			disconnect() {}
		},
	);
	ipc.invoke.mockReset();
	ipc.invoke.mockResolvedValue(undefined);
	target = document.createElement('div');
	document.body.appendChild(target);
});

afterEach(() => {
	if (app) void unmount(app);
	app = null;
	target.remove();
	vi.unstubAllGlobals();
});

function text(): string {
	return (target.textContent ?? '').replace(/\s+/g, ' ').trim();
}

describe('AgentBranch: the branch of a finished sub-agent', () => {
	it('shows its mission, its tool call, its answer, and the fixed bar in place of a composer', () => {
		const { conv } = finishedParent();
		const agent = conv.agents.get(AGENT)!;
		app = mount(AgentBranch, { target, props: { conversation: conv, agent } });
		flushSync();

		const shown = text();
		expect(shown).toContain('Chercher les occurrences');
		expect(shown).toContain('Read');
		expect(shown).toContain('La constante vaut un.');
		expect(shown).toContain('Vous ne pouvez pas écrire à un sous-agent !');
	});

	it('has no field and no form to write to', () => {
		const { conv } = finishedParent();
		app = mount(AgentBranch, { target, props: { conversation: conv, agent: conv.agents.get(AGENT)! } });
		flushSync();

		expect(target.querySelector('textarea')).toBeNull();
		expect(target.querySelector('input')).toBeNull();
		expect(target.querySelector('form')).toBeNull();
		expect(target.querySelector('[contenteditable="true"]')).toBeNull();
	});

	it('offers no revision on the mission, but still offers a copy', () => {
		const { conv } = finishedParent();
		app = mount(AgentBranch, { target, props: { conversation: conv, agent: conv.agents.get(AGENT)! } });
		flushSync();

		expect(target.querySelector('[aria-label="Modifier le message"]')).toBeNull();
		expect(target.querySelector('[aria-label="Copier le message"]')).not.toBeNull();
	});
});

describe('AgentBranch: a running sub-agent', () => {
	it('shows the mission while the agent works, before any answer', () => {
		const conv = new Conversation(openedSession(), PROJECT_ID);
		const feed = new EnvelopeFeed({ runId: RUN });
		conv.apply(feed.next({ type: 'run_started', prompt: 'Cherche', attachments: [], model: 'm' }, { at: T0 }));
		conv.apply(feed.next({ type: 'tool_started', tool_call_id: 'tc-A', name: 'Agent', input: {} }));
		conv.apply(feed.next({ type: 'agent_spawned', agent: spawnInfo({ agent_id: AGENT, tool_call_id: 'tc-A' }) }));

		app = mount(AgentBranch, { target, props: { conversation: conv, agent: conv.agents.get(AGENT)! } });
		flushSync();

		expect(text()).toContain('Chercher les occurrences');
		expect(text()).toContain('Vous ne pouvez pas écrire à un sous-agent !');
	});
});

describe('UserMessage: a read-only conversation offers no revision', () => {
	const item: UserItem = { kind: 'user', id: 'u-1', text: 'Bonjour', attachments: [] };

	function mountWith(readOnly: boolean) {
		const conv = new Conversation(openedSession(), PROJECT_ID);
		conv.readOnly = readOnly;
		app = mount(UserMessage, { target, props: { item, conversation: conv } });
		flushSync();
	}

	it('a conversation offers « Modifier le message » on its messages', () => {
		mountWith(false);
		expect(target.querySelector('[aria-label="Modifier le message"]')).not.toBeNull();
	});

	it('a branch does not offer it, and keeps the copy', () => {
		mountWith(true);
		expect(target.querySelector('[aria-label="Modifier le message"]')).toBeNull();
		expect(target.querySelector('[aria-label="Copier le message"]')).not.toBeNull();
	});
});
