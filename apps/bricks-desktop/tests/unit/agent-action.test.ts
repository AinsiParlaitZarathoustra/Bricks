// The second line of a sub-agent in the thread (agentView.ts agentAction): what it does now, from
// the tool calls the engine reports as running only. Pure: built from agent records.
import { describe, expect, it } from 'vitest';
import type { AgentInfo, AgentToolCall } from '../../src/lib/state/conversation.svelte';
import type { InstanceState } from '../../src/lib/ipc/engine';
import { agentAction } from '../../src/lib/features/agents/agentView';
import { T0, spawnInfo } from './fixtures';

function tool(name: string, input: unknown, status: AgentToolCall['status'] = 'running', n = 1): AgentToolCall {
	return { toolCallId: `atc-${name}-${n}`, name, input, status };
}

function agent(state: InstanceState, tools: AgentToolCall[] = [], overrides: Partial<AgentInfo> = {}): AgentInfo {
	return { id: 'ag-1', spawn: spawnInfo(), state, spawnedAt: T0, tools, execution: 1, ...overrides };
}

describe('agentAction: what a running sub-agent does now', () => {
	it('a running agent with no running tool says nothing', () => {
		expect(agentAction(agent('running'), false)).toBeNull();
		expect(agentAction(agent('running', [tool('Read', { file_path: '/a/b.ts' }, 'ok')]), false)).toBeNull();
	});

	it('a single Read names the file (its base name) and ends with an ellipsis', () => {
		expect(agentAction(agent('running', [tool('Read', { file_path: '/work/demo/src/calcul.ts' })]), false)).toBe(
			'Lecture de calcul.ts…'
		);
		expect(agentAction(agent('running', [tool('Read', { path: 'docs\\notes.md' })]), false)).toBe('Lecture de notes.md…');
	});

	it('a Read without a path says « Lecture »', () => {
		expect(agentAction(agent('running', [tool('Read', {})]), false)).toBe('Lecture…');
	});

	it('a single Bash shows the command, trimmed', () => {
		expect(agentAction(agent('running', [tool('Bash', { command: '  cargo test  ' })]), false)).toBe(
			'Commande : cargo test…'
		);
		expect(agentAction(agent('running', [tool('Bash', { command: '   ' })]), false)).toBe('Commande…');
	});

	it('other single tools use their French label, or their own name when unknown', () => {
		expect(agentAction(agent('running', [tool('Grep', { pattern: 'x' })]), false)).toBe('Recherche dans le code…');
		expect(agentAction(agent('running', [tool('Edit', { file_path: '/w/a.ts' })]), false)).toBe('Modification de a.ts…');
		expect(agentAction(agent('running', [tool('WebFetch', {})]), false)).toBe('Recherche sur le web…');
		expect(agentAction(agent('running', [tool('Mystery', {})]), false)).toBe('Mystery…');
	});

	it('several running reads say how many files', () => {
		const reads = [tool('Read', { file_path: 'a.ts' }, 'running', 1), tool('Read', { file_path: 'b.ts' }, 'running', 2)];
		expect(agentAction(agent('running', reads), false)).toBe('Lecture de 2 fichiers…');
	});

	it('several running edits of one kind say how many files; an edit and a write are counted as tools', () => {
		const edits = [tool('Edit', { file_path: 'a.ts' }, 'running', 1), tool('Edit', { file_path: 'b.ts' }, 'running', 2)];
		expect(agentAction(agent('running', edits), false)).toBe('Modification de 2 fichiers…');
		const editAndWrite = [tool('Edit', { file_path: 'a.ts' }, 'running', 1), tool('Write', { file_path: 'b.ts' }, 'running', 2)];
		expect(agentAction(agent('running', editAndWrite), false)).toBe('2 outils en cours…');
	});

	it('mixed running tools are counted, not named', () => {
		const mixed = [tool('Read', { file_path: 'a.ts' }, 'running', 1), tool('Bash', { command: 'ls' }, 'running', 2)];
		expect(agentAction(agent('running', mixed), false)).toBe('2 outils en cours…');
		const twoBash = [tool('Bash', { command: 'ls' }, 'running', 1), tool('Bash', { command: 'pwd' }, 'running', 2)];
		expect(agentAction(agent('running', twoBash), false)).toBe('2 outils en cours…');
	});

	it('only the running tools count: finished ones are ignored', () => {
		const tools = [
			tool('Read', { file_path: 'a.ts' }, 'ok', 1),
			tool('Bash', { command: 'cargo test' }, 'running', 2),
			tool('Grep', { pattern: 'x' }, 'error', 3)
		];
		expect(agentAction(agent('running', tools), false)).toBe('Commande : cargo test…');
	});

	it('awaiting an approval wins over the running tools', () => {
		expect(agentAction(agent('running', [tool('Bash', { command: 'rm x' })]), true)).toBe('En attente d’autorisation');
		expect(agentAction(agent('running', [], {}), true)).toBe('En attente d’autorisation');
	});

	it('a terminal agent has no action line, even when awaiting an approval or with a running tool', () => {
		for (const state of ['completed', 'incomplete', 'failed', 'interrupted', 'cancelled'] as const) {
			expect(agentAction(agent(state, [tool('Bash', { command: 'ls' })]), true)).toBeNull();
		}
	});

	it('a queued agent waiting for its worktree says so; without that reason it says nothing', () => {
		expect(agentAction(agent('queued', [], { reason: 'preparing its worktree' }), false)).toBe(
			'Préparation de son espace de travail…'
		);
		expect(agentAction(agent('created', [], { reason: 'preparing its worktree' }), false)).toBe(
			'Préparation de son espace de travail…'
		);
		expect(agentAction(agent('queued'), false)).toBeNull();
		expect(agentAction(agent('waiting_admission', [], { reason: 'max concurrency' }), false)).toBeNull();
	});
});
