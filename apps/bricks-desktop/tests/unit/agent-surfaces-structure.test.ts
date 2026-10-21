// Static checks (sources read, not rendered) of the sub-agent surfaces: the line in the thread,
// the inspector and its host, the removed agent list and instances view, and the one dispatch
// that opens the inspector. Same approach as copy-inspector-terminal-structure.test.ts.
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const SRC = fileURLToPath(new URL('../../src/', import.meta.url));
const src = (path: string) => readFileSync(join(SRC, path), 'utf8');

const LINE = 'lib/features/conversation/timeline/AgentLine.svelte';
const INSPECTOR = 'lib/features/agents/AgentInspector.svelte';
const HOST = 'lib/features/agents/AgentInspectorHost.svelte';
const DISPATCH = 'lib/features/commands/dispatch.ts';

/** Every text file under src/, as paths relative to src/. */
function textFiles(dir = SRC, prefix = ''): string[] {
	return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
		const rel = prefix + entry.name;
		if (entry.isDirectory()) return textFiles(join(dir, entry.name), `${rel}/`);
		return /\.(svelte|ts|js|json|md|html|css)$/.test(entry.name) ? [rel] : [];
	});
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

	it('opens the inspector of its agent on click', () => {
		expect(line).toContain('onclick={() => ui.openAgent(agent.id)}');
	});

	it('shows a resumed agent once, where it was resumed', () => {
		expect(line).toContain('item.execution !== agent.execution');
	});
});

describe('the sub-agent inspector', () => {
	const text = src(INSPECTOR);

	it('shows the three sections in order, then a collapsed « Détails »', () => {
		const result = text.indexOf('>Résultat</h3>');
		const files = text.indexOf('>Fichiers modifiés</h3>');
		const commands = text.indexOf('>Commandes</h3>');
		const details = text.indexOf('<Collapsible.Root');
		expect(result).toBeGreaterThan(-1);
		expect(files).toBeGreaterThan(result);
		expect(commands).toBeGreaterThan(files);
		expect(details).toBeGreaterThan(commands);
		expect(text).toMatch(/<Collapsible\.Trigger[\s\S]*?>\s*[\s\S]*?Détails\s*<\/Collapsible\.Trigger>/);
	});

	it('keeps the details behind the collapsible: mission, identity, model, workspace and tool calls', () => {
		const content = text.indexOf('<Collapsible.Content');
		const close = text.indexOf('</Collapsible.Content>');
		expect(content).toBeGreaterThan(-1);
		for (const title of ['>Mission</h3>', '>Identité</h3>', '>Modèle et raisonnement</h3>', '>Espace de travail</h3>', 'Appels d’outils']) {
			const at = text.indexOf(title);
			expect(at, title).toBeGreaterThan(content);
			expect(at, title).toBeLessThan(close);
		}
	});

	it('has no « Coût » any more', () => {
		expect(text).not.toContain('Coût');
	});

	it('offers a message form to a live agent, sent as an agent_control message', () => {
		expect(text).toContain('Message à l’agent');
		expect(text).toContain("{waiting && busy === 'message' ? 'Envoi…' : 'Envoyer'}");
		expect(text).toContain("void control('message', { agent_id: agent.id, message: text });");
		expect(text).toMatch(/\{#if live && agent\.state !== 'cancelling'\}\s*<form/);
	});

	it('keeps its markdown texts through MarkdownView', () => {
		expect(text).toContain('<MarkdownView source={spawn.task}');
		expect(text).toContain('<MarkdownView source={result.summary}');
	});
});

describe('the inspector host and the removed sub-agent list', () => {
	it('the host mounts the inspector of the agent chosen in the thread', () => {
		const host = src(HOST);
		expect(host).toContain('ui.agentId');
		expect(host).toContain('<AgentInspector {agent} {conversation} {now} onClose={() => ui.closeAgent()} />');
	});

	it('the agent list and the instances view are gone from the tree', () => {
		expect(existsSync(join(SRC, 'lib/features/agents/AgentList.svelte'))).toBe(false);
		expect(existsSync(join(SRC, 'lib/features/catalog/AgentInstancesView.svelte'))).toBe(false);
	});

	it('no file of src/ names the removed agent_instances view', () => {
		const offenders = textFiles().filter((file) => readFileSync(join(SRC, file), 'utf8').includes('agent_instances'));
		expect(offenders).toEqual([]);
	});

	it('the « / » dispatcher opens the sub-agent inspector through openAgent, not the removed view', () => {
		const dispatch = src(DISPATCH);
		expect(dispatch).toContain('host.openAgent?.(command.agent_id)');
		expect(dispatch).not.toContain("'agent_instances'");
	});
});
