// Static checks (sources read, not rendered) of the sub-agent surfaces: the line in the thread, the
// branch that replaces the former inspector (its component, its title bar, App.svelte mounting it),
// the removed inspector, message form and tool names, and the one dispatch that opens a branch.
// Same approach as copy-inspector-terminal-structure.test.ts.
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { bound, call } from './french-keys';

const SRC = fileURLToPath(new URL('../../src/', import.meta.url));
const src = (path: string) => readFileSync(join(SRC, path), 'utf8');

const LINE = 'lib/features/conversation/timeline/AgentLine.svelte';
const BRANCH = 'lib/features/agents/AgentBranch.svelte';
const TITLE = 'lib/features/conversation/ConversationTitle.svelte';
const APP = 'App.svelte';
const DISPATCH = 'lib/features/commands/dispatch.ts';

/** Every text file under src/, as paths relative to src/. */
function textFiles(dir = SRC, prefix = ''): string[] {
	return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
		const rel = prefix + entry.name;
		if (entry.isDirectory()) return textFiles(join(dir, entry.name), `${rel}/`);
		return /\.(svelte|ts|js|json|md|html|css)$/.test(entry.name) ? [rel] : [];
	});
}

/** The files of src/ whose text matches `pattern`, except the ones listed in `allowed`. */
function offendersOf(pattern: RegExp, allowed: readonly string[] = []): string[] {
	return textFiles()
		.filter((file) => !allowed.includes(file))
		.filter((file) => pattern.test(readFileSync(join(SRC, file), 'utf8')));
}

describe('the sub-agent line in the thread', () => {
	const line = src(LINE);

	it('shows no diff counts, no model, no reasoning and no cost', () => {
		// The component's own comments say what it leaves out: only the markup and script count.
		const code = line.replace(/^\s*\/\/.*$/gm, '');
		expect(code).not.toMatch(/DiffStat|\+\{|−\{|\+\d|−\d/);
		for (const word of ['model', 'reasoning', 'cost', 'Coût', 'usd', 'USD', 'formatUsd']) {
			expect(code, word).not.toContain(word);
		}
	});

	it('shows the dot, the profile, the description and the duration, and a second action line', () => {
		expect(line).toContain('<StatusDot');
		expect(line).toContain('{agent.spawn.profile}');
		expect(line).toContain('{description}');
		expect(line).toContain('formatDuration(agentDurationMs(agent, now))');
		expect(line).toContain('agentAction(agent, awaitingApproval)');
	});

	it('opens the branch of its agent on click', () => {
		expect(line).toContain('onclick={() => ui.openAgent(agent.id)}');
	});

	it('shows a resumed agent once, where it was resumed', () => {
		expect(line).toContain('item.execution !== agent.execution');
	});
});

describe('the branch of a sub-agent (AgentBranch.svelte)', () => {
	const text = src(BRANCH);

	it('renders the same timeline as the conversation, on the branch built from the agent steps', () => {
		expect(text).toContain("import ConversationTimeline from '$lib/features/conversation/ConversationTimeline.svelte'");
		expect(text).toContain('<ConversationTimeline conversation={branch} />');
		expect(text).toContain('agentBranch(conversation, agent)');
	});

	it('builds the branch once and feeds it the steps as the agent log grows', () => {
		expect(text).toContain('agentBranch(conversation, agent)');
		expect(text).toContain('applyAgentSteps(branch, agent, applied)');
	});

	it('has nothing to write to: a fixed bar says so, and there is no field, form or command', () => {
		expect(text).toContain(call('Vous ne pouvez pas écrire à un sous-agent !'));
		expect(text).not.toContain('<textarea');
		expect(text).not.toContain('<form');
		expect(text).not.toContain('control(');
	});
});

describe('the title bar in a branch (ConversationTitle.svelte)', () => {
	const text = src(TITLE);

	it('reads the agent of the open branch, and names it after the conversation title', () => {
		expect(text).toContain("import { agentName } from '$lib/features/agents/branch'");
		expect(text).toContain('const branch = $derived(ui.agentId ? (conversation?.agents.get(ui.agentId) ?? null) : null)');
		expect(text).toContain('{agentName(branch)}');
	});

	it('leads back to the conversation: its title is a button that closes the branch, then a slash', () => {
		expect(text).toContain('onclick={() => ui.closeAgent()}>{title}</button');
		expect(text).toContain('>/</span>');
		const back = text.indexOf('onclick={() => ui.closeAgent()}');
		const slash = text.indexOf('>/</span>', back);
		const name = text.indexOf('{agentName(branch)}');
		expect(back).toBeGreaterThan(-1);
		expect(slash).toBeGreaterThan(back);
		expect(name).toBeGreaterThan(slash);
	});

	it('hides the rename pencil in a branch, and the running dot follows the branch', () => {
		const pencil = text.indexOf(bound('aria-label', 'Modifier le titre'));
		expect(pencil).toBeGreaterThan(-1);
		expect(text.slice(0, pencil)).toContain('{#if conversation && !editing && !branch}');
		expect(text).toContain('{#if branch ? !branch.endedAt : conversation?.running}');
	});
});

describe('App.svelte: the branch is mounted in the centre', () => {
	const text = src(APP);

	it('mounts AgentBranch for the agent of the open branch, above the conversation', () => {
		expect(text).toContain("import AgentBranch from '$lib/features/agents/AgentBranch.svelte'");
		const branch = text.indexOf('{#if app.current && branchAgent}');
		expect(branch).toBeGreaterThan(-1);
		expect(text.indexOf('<AgentBranch conversation={app.current} agent={branchAgent} />')).toBeGreaterThan(branch);
	});

	it('keeps the conversation mounted but hidden while a branch is open', () => {
		expect(text).toContain("branchAgent && 'hidden'");
		expect(text).toContain('Kept mounted under a branch');
		const branch = text.indexOf('<AgentBranch');
		expect(text.indexOf("branchAgent && 'hidden'")).toBeGreaterThan(branch);
	});

	it('closes the branch when the session changes', () => {
		expect(text).toContain('if (id !== branchSession) {');
		expect(text).toContain('ui.closeAgent()');
	});

	it('no longer mounts the inspector host', () => {
		expect(text).not.toContain('AgentInspectorHost');
	});
});

describe('the removed inspector, message form and tool names', () => {
	it('the inspector and its host are gone from the tree', () => {
		expect(existsSync(join(SRC, 'lib/features/agents/AgentInspector.svelte'))).toBe(false);
		expect(existsSync(join(SRC, 'lib/features/agents/AgentInspectorHost.svelte'))).toBe(false);
	});

	it('the agent list and the instances view are gone from the tree', () => {
		expect(existsSync(join(SRC, 'lib/features/agents/AgentList.svelte'))).toBe(false);
		expect(existsSync(join(SRC, 'lib/features/catalog/AgentInstancesView.svelte'))).toBe(false);
	});

	it('no file of src/ names the removed agent_instances view', () => {
		expect(offendersOf(/agent_instances/)).toEqual([]);
	});

	it('no file of src/ names AgentInspector', () => {
		expect(offendersOf(/AgentInspector/)).toEqual([]);
	});

	it('no file of src/ names the SendMessage tool', () => {
		expect(offendersOf(/SendMessage/)).toEqual([]);
	});

	it('no file of src/ uses « Agents » as a tool name (a case, a comparison, a key)', () => {
		// The labels « Agents » of adkAccess.ts (a group) and of ConfigView.svelte (a section) are text, not the tool.
		const TOOL_NAME_USE = /case ['"`]Agents['"`]|[!=]==? ?['"`]Agents['"`]|['"`]Agents['"`]\s*:/;
		expect(TOOL_NAME_USE.test("case 'Agents':")).toBe(true);
		expect(TOOL_NAME_USE.test("label: 'Agents'")).toBe(false);
		expect(offendersOf(TOOL_NAME_USE)).toEqual([]);
	});

	it('no file of src/ still offers the message form of an agent', () => {
		expect(offendersOf(/Message à l’agent/)).toEqual([]);
		expect(offendersOf(/control\('message'/)).toEqual([]);
	});

	it('the « / » dispatcher opens the branch through openAgent, not the removed view', () => {
		const dispatch = src(DISPATCH);
		expect(dispatch).toContain('host.openAgent?.(command.agent_id)');
		expect(dispatch).not.toContain("'agent_instances'");
	});
});
