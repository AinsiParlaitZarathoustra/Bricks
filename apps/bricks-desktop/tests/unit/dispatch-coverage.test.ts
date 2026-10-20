// Coverage of the `/` dispatcher.
//
// Every view of the generated `SlashPresent` binding and every engine `Command` type is listed
// below with the desktop effect it must produce. The two lists are also read from the generated
// binding and from src/lib/ipc/engine.ts at test time, so a new Rust variant that this file does
// not describe fails here instead of falling through. Nothing may reach the model as a prompt:
// a `/` line is only ever sent as a `submit` when it is a `/skill` (or an engine submit).
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import type { SlashAction } from '../../src/lib/ipc/bindings/SlashAction';
import type { SlashPresent } from '../../src/lib/ipc/bindings/SlashPresent';
import type { Command, PromptBlock } from '../../src/lib/ipc/engine';
import {
	runSlashLine,
	type SlashHost,
	type SlashOutcome
} from '../../src/lib/features/commands/dispatch';

interface Call {
	name: string;
	args: unknown[];
}

const SESSION = 's-1';

/** Options of the fake desktop: the open conversation and the engine's answers. */
interface Options {
	session?: string | null;
	sendError?: string | null;
	resumeError?: string | null;
	attachment?: { blocks: PromptBlock[] } | { error: string };
}

/** A host that records every call. `action` is what the parser returns for the line. */
function fakeHost(action: SlashAction, options: Options = {}) {
	const calls: Call[] = [];
	const rec = (name: string, ...args: unknown[]) => {
		calls.push({ name, args });
	};
	let session: string | null = options.session === undefined ? SESSION : options.session;
	const host: SlashHost = {
		async parse(line) {
			rec('parse', line);
			return action;
		},
		currentSession: () => session,
		async ensureSession() {
			rec('ensureSession');
			session ??= 'new-session';
			return { sessionId: session };
		},
		mark(sessionId) {
			rec('mark', sessionId);
			return { sentAt: 1000, runsBefore: 2 };
		},
		async send(sessionId, command, display) {
			rec('send', sessionId, command, display);
			return options.sendError ?? null;
		},
		async setModel(model, reasoning) {
			rec('setModel', model, reasoning);
			return null;
		},
		async setEffort(profile) {
			rec('setEffort', profile);
			return null;
		},
		openPicker: (picker) => rec('openPicker', picker),
		open: (view, args) => rec('open', view, args),
		attachmentBlocks() {
			rec('attachmentBlocks');
			return options.attachment ?? { blocks: [] };
		},
		async resume(sessionId) {
			rec('resume', sessionId);
			return options.resumeError ?? null;
		},
		showChanges: () => rec('showChanges'),
		async attach(kind, path) {
			rec('attach', kind, path);
			return null;
		},
		async quit() {
			rec('quit');
			return null;
		}
	};
	return { host, calls };
}

/** Calls after the parser call. */
function effects(calls: Call[]): Call[] {
	return calls.slice(1);
}

/** The recorded call of `host.open(view, args)`: the dispatcher always passes two arguments. */
function open(view: string, args?: unknown): Call {
	return { name: 'open', args: [view, args] };
}

function sendsOfType(calls: Call[], type: string): number {
	return calls.filter((c) => c.name === 'send' && (c.args[1] as Command).type === type).length;
}

const read = (relative: string) =>
	readFileSync(fileURLToPath(new URL(relative, import.meta.url)), 'utf8');

/** The `view` tags of the generated `SlashPresent` binding. */
function bindingViews(): string[] {
	return [...read('../../src/lib/ipc/bindings/SlashPresent.ts').matchAll(/"view": "([a-z_]+)"/g)]
		.map((m) => m[1])
		.sort();
}

/** The `type` tags of the engine `Command` union (src/lib/ipc/engine.ts). */
function engineCommandTypes(): string[] {
	const block = read('../../src/lib/ipc/engine.ts').match(
		/export type Command =([\s\S]*?)export type Activity/
	);
	if (!block) throw new Error('the Command union was not found in engine.ts');
	return [...block[1].matchAll(/type: '([a-z_]+)'/g)].map((m) => m[1]).sort();
}

// ─── Presentations ─────────────────────────────────────────────────────────

/** The desktop effect of each view, after the parser call. */
const PRESENT: Record<SlashPresent['view'], { present: SlashPresent; calls: Call[] }> = {
	model_picker: {
		present: { view: 'model_picker' },
		calls: [{ name: 'openPicker', args: ['model'] }]
	},
	effort_picker: {
		present: { view: 'effort_picker' },
		calls: [{ name: 'openPicker', args: ['effort'] }]
	},
	memory: { present: { view: 'memory' }, calls: [open('memory')] },
	context: { present: { view: 'context' }, calls: [open('context')] },
	context_details: {
		present: { view: 'context_details' },
		calls: [open('context', { details: true })]
	},
	cost: { present: { view: 'cost' }, calls: [open('cost')] },
	cost_details: {
		present: { view: 'cost_details' },
		calls: [open('cost', { details: true })]
	},
	session: { present: { view: 'session' }, calls: [open('session')] },
	session_picker: {
		present: { view: 'session_picker' },
		calls: [open('sessions')]
	},
	resume: {
		present: { view: 'resume', session_id: '2026-x' },
		calls: [{ name: 'resume', args: ['2026-x'] }]
	},
	diff: { present: { view: 'diff' }, calls: [{ name: 'showChanges', args: [] }] },
	tools: { present: { view: 'tools' }, calls: [open('tools')] },
	mcp: { present: { view: 'mcp' }, calls: [open('mcp')] },
	config: { present: { view: 'config' }, calls: [open('config')] },
	help: { present: { view: 'help' }, calls: [open('help')] },
	quit: { present: { view: 'quit' }, calls: [{ name: 'quit', args: [] }] },
	attach: {
		present: { view: 'attach', kind: 'file', path: 'Mon dossier/é.txt' },
		calls: [{ name: 'attach', args: ['file', 'Mon dossier/é.txt'] }]
	}
};

describe('SlashPresent: every view of the binding has a desktop effect', () => {
	it('lists exactly the views of the generated binding', () => {
		expect(Object.keys(PRESENT).sort()).toEqual(bindingViews());
	});

	it.each(Object.keys(PRESENT))('view %s opens its view with no engine call', async (view) => {
		const { present, calls } = PRESENT[view as SlashPresent['view']];
		const fake = fakeHost({ kind: 'present', present });
		const outcome = await runSlashLine(`/${view}`, fake.host);
		expect(effects(fake.calls)).toEqual(calls);
		expect(outcome).toEqual({ kind: 'done' });
	});

	it('shows the refusal of /resume <id> when the desktop cannot reopen the session', async () => {
		const fake = fakeHost(
			{ kind: 'present', present: { view: 'resume', session_id: 'gone' } },
			{ resumeError: 'session introuvable' }
		);
		const outcome = await runSlashLine('/resume gone', fake.host);
		expect(outcome).toEqual({ kind: 'refused', message: 'session introuvable' });
	});

	it('refuses an attachment kind it does not know, without attaching', async () => {
		const fake = fakeHost({
			kind: 'present',
			present: { view: 'attach', kind: 'video', path: 'a.mp4' }
		} as unknown as SlashAction);
		const outcome = await runSlashLine('/video a.mp4', fake.host);
		expect(outcome.kind).toBe('refused');
		expect(fake.calls.map((c) => c.name)).not.toContain('attach');
	});
});

// ─── Engine commands ───────────────────────────────────────────────────────

/** One sample command per engine type, as the registry (or the dispatcher) can produce it. */
const ENGINE: { type: string; registry: boolean; command: Command }[] = [
	{ type: 'submit', registry: true, command: { type: 'submit', prompt: { blocks: [{ kind: 'text', text: 'Bonjour' }], skills: ['pdf'] } } },
	{ type: 'cancel', registry: false, command: { type: 'cancel' } },
	{ type: 'set_model', registry: true, command: { type: 'set_model', model: 'test/b', reasoning: 'deep' } },
	{ type: 'set_reasoning', registry: true, command: { type: 'set_reasoning', profile: null } },
	{ type: 'compact', registry: true, command: { type: 'compact' } },
	{ type: 'clear_context', registry: true, command: { type: 'clear_context' } },
	{ type: 'resume', registry: true, command: { type: 'resume', session_id: '2026-x' } },
	{ type: 'search', registry: true, command: { type: 'search', text: 'x', regex: false } },
	{ type: 'list_agent_profiles', registry: true, command: { type: 'list_agent_profiles', query: 'rust', page: 0 } },
	{ type: 'reload_agent_profiles', registry: true, command: { type: 'reload_agent_profiles' } },
	{ type: 'agent_control', registry: true, command: { type: 'agent_control', action: 'list' } },
	{ type: 'list_skills', registry: true, command: { type: 'list_skills', query: 'pdf', page: 0 } },
	{ type: 'reload_skills', registry: true, command: { type: 'reload_skills' } },
	{ type: 'trust_project_skills', registry: true, command: { type: 'trust_project_skills', allow: true } },
	{ type: 'approve', registry: false, command: { type: 'approve', approval_id: 'ap-1', decision: 'allow' } }
];

/** Engine types a `/` line can produce (crates/bricks-agent/src/control/slash.rs). */
const REGISTRY_ENGINE_TYPES = [
	'agent_control',
	'clear_context',
	'compact',
	'list_agent_profiles',
	'list_skills',
	'reload_agent_profiles',
	'reload_skills',
	'resume',
	'search',
	'set_model',
	'set_reasoning',
	'submit',
	'trust_project_skills'
];

describe('engine Command: every type of the union is listed', () => {
	it('lists exactly the Command types of engine.ts', () => {
		expect(ENGINE.map((e) => e.type).sort()).toEqual(engineCommandTypes());
	});

	it('lists the registry-producible types the spec names', () => {
		const producible = ENGINE.filter((e) => e.registry)
			.map((e) => e.type)
			.sort();
		expect(producible).toEqual([...REGISTRY_ENGINE_TYPES].sort());
	});
});

describe('engine Command: desktop effect of each type', () => {
	const engine = (command: Command): SlashAction => ({ kind: 'engine', command });

	it('set_model is sent to the open session, or kept for the first send without one', async () => {
		const open = fakeHost(engine({ type: 'set_model', model: 'test/b', reasoning: 'deep' }));
		const sent = await runSlashLine('/model test/b deep', open.host);
		expect(sent).toEqual({
			kind: 'sent',
			sessionId: SESSION,
			sentAt: 1000,
			runsBefore: 2,
			watch: { kind: 'none' }
		});

		const closed = fakeHost(engine({ type: 'set_model', model: 'test/b', reasoning: 'deep' }), {
			session: null
		});
		expect(await runSlashLine('/model test/b deep', closed.host)).toEqual({ kind: 'done' });
		expect(effects(closed.calls)).toEqual([{ name: 'setModel', args: ['test/b', 'deep'] }]);
	});

	it('set_model without a model opens the picker and sends nothing', async () => {
		const fake = fakeHost(engine({ type: 'set_model' }));
		expect(await runSlashLine('/model', fake.host)).toEqual({ kind: 'done' });
		expect(effects(fake.calls)).toEqual([{ name: 'openPicker', args: ['model'] }]);
	});

	it('set_reasoning with null resets the profile: sent, or kept locally without a session', async () => {
		const open = fakeHost(engine({ type: 'set_reasoning', profile: null }));
		await runSlashLine('/effort default', open.host);
		expect(sendsOfType(open.calls, 'set_reasoning')).toBe(1);
		expect(open.calls.find((c) => c.name === 'send')?.args[1]).toEqual({
			type: 'set_reasoning',
			profile: null
		});

		const closed = fakeHost(engine({ type: 'set_reasoning', profile: null }), { session: null });
		expect(await runSlashLine('/effort default', closed.host)).toEqual({ kind: 'done' });
		expect(effects(closed.calls)).toEqual([{ name: 'setEffort', args: [null] }]);
	});

	it('a refused set_reasoning keeps the engine message', async () => {
		const fake = fakeHost(engine({ type: 'set_reasoning', profile: 'x' }), {
			sendError: 'profil inconnu'
		});
		expect(await runSlashLine('/effort x', fake.host)).toEqual({
			kind: 'refused',
			message: 'profil inconnu'
		});
	});

	it('search opens the search view with the query, then sends it', async () => {
		const fake = fakeHost(engine({ type: 'search', text: 'x', regex: true }));
		const outcome = await runSlashLine('/search re:x', fake.host);
		expect(outcome.kind).toBe('sent');
		expect(effects(fake.calls).map((c) => c.name)).toEqual(['open', 'mark', 'send']);
		expect(fake.calls[1]).toEqual({ name: 'open', args: ['search', { query: 'x', regex: true }] });
	});

	it('commands that need a conversation are refused without one, and create none', async () => {
		const closed = fakeHost(engine({ type: 'search', text: 'x', regex: false }), { session: null });
		expect(await runSlashLine('/search x', closed.host)).toEqual({
			kind: 'refused',
			message: 'Aucune conversation ouverte.'
		});
		expect(effects(closed.calls)).toEqual([]);

		const skills = fakeHost(engine({ type: 'list_skills', query: '', page: 0 }), { session: null });
		expect(await runSlashLine('/skills', skills.host)).toEqual({
			kind: 'refused',
			message: 'Aucune conversation ouverte.'
		});
		expect(sendsOfType(skills.calls, 'list_skills')).toBe(0);
	});

	it('list_agent_profiles and reload_agent_profiles open the catalogue, then send', async () => {
		const list = fakeHost(engine({ type: 'list_agent_profiles', query: 'rust', page: 0 }));
		await runSlashLine('/agents rust', list.host);
		expect(list.calls.find((c) => c.name === 'open')).toEqual({
			name: 'open',
			args: ['agents', { query: 'rust' }]
		});
		expect(sendsOfType(list.calls, 'list_agent_profiles')).toBe(1);

		const reload = fakeHost(engine({ type: 'reload_agent_profiles' }));
		await runSlashLine('/agents reload', reload.host);
		expect(reload.calls.find((c) => c.name === 'open')).toEqual({
			name: 'open',
			args: ['agents', {}]
		});
		expect(sendsOfType(reload.calls, 'reload_agent_profiles')).toBe(1);
	});

	it('/agents running lists the instances in the open session only', async () => {
		const fake = fakeHost(engine({ type: 'agent_control', action: 'list' }));
		await runSlashLine('/agents running', fake.host);
		expect(fake.calls.find((c) => c.name === 'open')?.args[0]).toBe('agent_instances');
		expect(sendsOfType(fake.calls, 'agent_control')).toBe(1);

		const closed = fakeHost(engine({ type: 'agent_control', action: 'list' }), { session: null });
		expect(await runSlashLine('/agents running', closed.host)).toEqual({
			kind: 'refused',
			message: 'Ouvrez une conversation pour utiliser cette commande.'
		});
		expect(effects(closed.calls)).toEqual([]);
	});

	it.each([
		['inspect_changes', 'inspect'],
		['apply_changes', 'apply'],
		['discard_changes', 'discard']
	])('%s opens the confirmation view and sends nothing', async (action, view) => {
		const fake = fakeHost(
			engine({ type: 'agent_control', action, changeset_id: 'cs-1' })
		);
		expect(await runSlashLine(`/changes cs-1 ${view}`, fake.host)).toEqual({ kind: 'done' });
		expect(effects(fake.calls)).toEqual([
			{ name: 'open', args: ['changeset', { changesetId: 'cs-1', action: view }] }
		]);
	});

	it('/changes without an open session is refused before opening anything', async () => {
		const fake = fakeHost(
			engine({ type: 'agent_control', action: 'apply_changes', changeset_id: 'cs-1' }),
			{ session: null }
		);
		expect(await runSlashLine('/changes cs-1 apply', fake.host)).toEqual({
			kind: 'refused',
			message: 'Ouvrez une conversation pour voir ses modifications isolées.'
		});
		expect(effects(fake.calls)).toEqual([]);
	});

	it('/jobs lists the engine jobs of the session; /jobs stop sends the stop', async () => {
		const list = fakeHost(engine({ type: 'agent_control', action: 'jobs' }));
		await runSlashLine('/jobs', list.host);
		expect(list.calls.find((c) => c.name === 'open')).toEqual({
			name: 'open',
			args: ['jobs', { action: 'jobs', jobId: undefined }]
		});
		expect(sendsOfType(list.calls, 'agent_control')).toBe(1);

		const stop = fakeHost(engine({ type: 'agent_control', action: 'stop_job', job_id: 'job-7' }));
		await runSlashLine('/jobs stop job-7', stop.host);
		expect(stop.calls.find((c) => c.name === 'open')).toEqual({
			name: 'open',
			args: ['jobs', { action: 'stop_job', jobId: 'job-7' }]
		});
		expect(stop.calls.find((c) => c.name === 'send')?.args[1]).toEqual({
			type: 'agent_control',
			action: 'stop_job',
			job_id: 'job-7'
		});
	});

	it.each([
		['list_skills', { type: 'list_skills', query: 'pdf', page: 0 } as Command],
		['reload_skills', { type: 'reload_skills' } as Command],
		['trust_project_skills', { type: 'trust_project_skills', allow: false } as Command]
	])('%s opens the skills view, then sends the command', async (_name, command) => {
		const fake = fakeHost(engine(command));
		const outcome = await runSlashLine('/skills', fake.host);
		expect(outcome.kind).toBe('sent');
		expect(fake.calls.find((c) => c.name === 'open')?.args[0]).toBe('skills');
		expect(sendsOfType(fake.calls, command.type)).toBe(1);
	});

	it('submit from /skill checks the attachments first, then sends with the skill', async () => {
		const fake = fakeHost(
			engine({ type: 'submit', prompt: { blocks: [{ kind: 'text', text: 'Extrais' }], skills: ['pdf'] } }),
			{ attachment: { blocks: [{ kind: 'file', path: 'a.pdf' }] } }
		);
		const outcome = await runSlashLine('/skill pdf Extrais', fake.host);
		expect(outcome).toEqual({
			kind: 'sent',
			sessionId: SESSION,
			sentAt: 1000,
			runsBefore: 2,
			watch: { kind: 'skill', name: 'pdf' }
		});
		expect(effects(fake.calls).map((c) => c.name)).toEqual([
			'attachmentBlocks',
			'ensureSession',
			'mark',
			'send'
		]);
		expect(fake.calls.find((c) => c.name === 'send')?.args[1]).toEqual({
			type: 'submit',
			prompt: {
				blocks: [
					{ kind: 'text', text: 'Extrais' },
					{ kind: 'file', path: 'a.pdf' }
				],
				skills: ['pdf']
			}
		});
		// The message shows the command as typed, not the request.
		expect(fake.calls.find((c) => c.name === 'send')?.args[2]).toBe('/skill pdf');
	});

	it('a blocked attachment stops a submit before any session is created', async () => {
		const fake = fakeHost(
			engine({ type: 'submit', prompt: { blocks: [{ kind: 'text', text: 'Hop' }] } }),
			{ attachment: { error: 'pièce jointe introuvable' } }
		);
		expect(await runSlashLine('/skill pdf Hop', fake.host)).toEqual({
			kind: 'refused',
			message: 'pièce jointe introuvable'
		});
		expect(effects(fake.calls).map((c) => c.name)).toEqual(['attachmentBlocks']);
	});

	it('compact is sent and watched; without a session it is refused', async () => {
		const open = fakeHost(engine({ type: 'compact' }));
		expect(await runSlashLine('/compact', open.host)).toEqual({
			kind: 'sent',
			sessionId: SESSION,
			sentAt: 1000,
			runsBefore: 2,
			watch: { kind: 'compact' }
		});

		const closed = fakeHost(engine({ type: 'compact' }), { session: null });
		expect(await runSlashLine('/compact', closed.host)).toEqual({
			kind: 'refused',
			message: 'Ouvrez une conversation pour compacter son contexte.'
		});
		expect(effects(closed.calls)).toEqual([]);
	});

	it('clear_context opens its explanation and sends nothing', async () => {
		const fake = fakeHost(engine({ type: 'clear_context' }));
		expect(await runSlashLine('/clear', fake.host)).toEqual({ kind: 'done' });
		expect(effects(fake.calls)).toEqual([open('clear')]);

		const closed = fakeHost(engine({ type: 'clear_context' }), { session: null });
		expect(await runSlashLine('/clear', closed.host)).toEqual({
			kind: 'refused',
			message: 'Ouvrez une conversation pour vider son contexte.'
		});
	});

	it('an engine resume goes through the desktop open path, never a send', async () => {
		const fake = fakeHost(engine({ type: 'resume', session_id: '2026-x' }));
		expect(await runSlashLine('/resume 2026-x', fake.host)).toEqual({ kind: 'done' });
		expect(effects(fake.calls)).toEqual([{ name: 'resume', args: ['2026-x'] }]);
	});

	it('cancel is sent to the open session; without one it is refused', async () => {
		const open = fakeHost({ kind: 'engine', command: { type: 'cancel' } });
		expect(await runSlashLine('/cancel', open.host)).toMatchObject({ kind: 'sent' });
		expect(sendsOfType(open.calls, 'cancel')).toBe(1);

		const closed = fakeHost({ kind: 'engine', command: { type: 'cancel' } }, { session: null });
		expect(await runSlashLine('/cancel', closed.host)).toEqual({
			kind: 'refused',
			message: 'Aucune génération en cours.'
		});
	});

	it('approve is refused in the dispatcher: approvals are answered in the conversation', async () => {
		const fake = fakeHost({
			kind: 'engine',
			command: { type: 'approve', approval_id: 'ap-1', decision: 'allow' }
		});
		expect(await runSlashLine('/approve', fake.host)).toEqual({
			kind: 'refused',
			message: 'Les demandes d’approbation se traitent dans la conversation.'
		});
		expect(fake.calls.map((c) => c.name)).toEqual(['parse']);
	});
});

// ─── No fallthrough ────────────────────────────────────────────────────────

describe('no `/` line falls through to a prompt', () => {
	const cases: { label: string; action: SlashAction }[] = [
		...Object.entries(PRESENT).map(([view, c]) => ({
			label: `present ${view}`,
			action: { kind: 'present', present: c.present } as SlashAction
		})),
		...ENGINE.map((e) => ({
			label: `engine ${e.type}`,
			action: { kind: 'engine', command: e.command } as SlashAction
		}))
	];

	for (const session of [SESSION, null] as const) {
		const where = session === null ? 'without an open session' : 'with an open session';

		it.each(cases.map((c) => ({ label: c.label, action: c.action })))(
			`$label, ${where}: never a submit unless it is one, never the unknown-command fallback`,
			async ({ label, action }) => {
				const fake = fakeHost(action, { session });
				const outcome: SlashOutcome = await runSlashLine('/x', fake.host);
				const isSubmit = (action as { command?: Command }).command?.type === 'submit';
				expect(sendsOfType(fake.calls, 'submit'), label).toBe(isSubmit ? 1 : 0);
				if (outcome.kind === 'refused') {
					expect(outcome.message, label).not.toMatch(/^Commande inconnue/);
				}
			}
		);
	}
});

describe('unknown actions are refused, never sent', () => {
	it('an unknown view is refused and nothing is sent', async () => {
		const fake = fakeHost({ kind: 'present', present: { view: 'future_view' } } as unknown as SlashAction);
		const outcome = await runSlashLine('/future', fake.host);
		expect(outcome.kind).toBe('refused');
		expect(sendsOfType(fake.calls, 'submit')).toBe(0);
		expect(fake.calls.map((c) => c.name)).toEqual(['parse']);
	});

	it('an unknown engine type is refused and nothing is sent', async () => {
		const fake = fakeHost({ kind: 'engine', command: { type: 'future_command' } } as unknown as SlashAction);
		const outcome = await runSlashLine('/future', fake.host);
		expect(outcome.kind).toBe('refused');
		expect(fake.calls.map((c) => c.name)).toEqual(['parse']);
	});
});
