// `/` dispatch: each parsed action becomes the right desktop calls, in the
// right order, with the right outcome. A fake host records every call.
import { describe, expect, it } from 'vitest';
import type { SlashAction } from '../../src/lib/ipc/bindings/SlashAction';
import type { SlashPresent } from '../../src/lib/ipc/bindings/SlashPresent';
import type { Command, PromptBlock } from '../../src/lib/ipc/engine';
import {
	runSlashLine,
	type SlashHost,
	type SlashOutcome
} from '../../src/lib/features/commands/dispatch';

interface Options {
	action?: SlashAction;
	parseError?: string;
	session?: string | null;
	ensureError?: string;
	sendError?: string | null;
	setModelError?: string | null;
	resumeError?: string | null;
	attachment?: { blocks: PromptBlock[] } | { error: string };
}

interface Call {
	name: string;
	args: unknown[];
}

/** A host that records its calls; `session` is the open conversation, if any. */
function fakeHost(options: Options = {}) {
	const calls: Call[] = [];
	const rec = (name: string, ...args: unknown[]) => calls.push({ name, args });
	let session: string | null = options.session ?? null;
	const host: SlashHost = {
		async parse(line) {
			rec('parse', line);
			if (options.parseError) throw new Error(options.parseError);
			return options.action as SlashAction;
		},
		currentSession: () => session,
		async ensureSession() {
			rec('ensureSession');
			if (options.ensureError) return { error: options.ensureError };
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
			return options.setModelError ?? null;
		},
		async setEffort(profile) {
			rec('setEffort', profile);
			return null;
		},
		openPicker: (picker) => rec('openPicker', picker),
		open: (view, args) => rec('open', view, args),
		openAgent: (agentId) => rec('openAgent', agentId),
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
	return { host, calls, names: () => calls.map((c) => c.name) };
}

const present = (p: SlashPresent): SlashAction => ({ kind: 'present', present: p });
const engine = (command: Command): SlashAction => ({ kind: 'engine', command });

async function run(options: Options, line = '/x') {
	const fake = fakeHost({ ...options });
	const outcome: SlashOutcome = await runSlashLine(line, fake.host);
	return { outcome, ...fake };
}

describe('runSlashLine: parsing', () => {
	it('keeps the parser message and sends nothing when the line is invalid', async () => {
		const r = await run({ parseError: 'commande inconnue : /zzz' }, '/zzz');
		expect(r.outcome).toEqual({ kind: 'refused', message: 'commande inconnue : /zzz' });
		expect(r.names()).toEqual(['parse']);
	});
});

describe('present actions', () => {
	it('opens the model picker without any engine call', async () => {
		const r = await run({ action: present({ view: 'model_picker' }) });
		expect(r.outcome).toEqual({ kind: 'done' });
		expect(r.calls).toEqual([
			{ name: 'parse', args: ['/x'] },
			{ name: 'openPicker', args: ['model'] }
		]);
	});

	it('opens the context view with details for /context details', async () => {
		const r = await run({ action: present({ view: 'context_details' }) });
		expect(r.calls.at(-1)).toEqual({ name: 'open', args: ['context', { details: true }] });
	});

	it('opens the cost view without details for /cost', async () => {
		const r = await run({ action: present({ view: 'cost' }) });
		expect(r.calls.at(-1)).toEqual({ name: 'open', args: ['cost', undefined] });
	});

	it('shows the changes for /diff', async () => {
		const r = await run({ action: present({ view: 'diff' }) });
		expect(r.outcome).toEqual({ kind: 'done' });
		expect(r.names().at(-1)).toBe('showChanges');
	});

	it('reopens a session by id and keeps the refusal', async () => {
		const ok = await run({ action: present({ view: 'resume', session_id: 'sess-1' }) });
		expect(ok.calls.at(-1)).toEqual({ name: 'resume', args: ['sess-1'] });
		expect(ok.outcome).toEqual({ kind: 'done' });

		const refused = await run({
			action: present({ view: 'resume', session_id: 'sess-2' }),
			resumeError: 'Session introuvable.'
		});
		expect(refused.outcome).toEqual({ kind: 'refused', message: 'Session introuvable.' });
	});

	it('hands /quit to the host', async () => {
		const r = await run({ action: present({ view: 'quit' }) });
		expect(r.names().at(-1)).toBe('quit');
	});

	it('attaches the path with its kind', async () => {
		const r = await run({ action: present({ view: 'attach', kind: 'image', path: '/tmp/Mon dossier/é.png' }) });
		expect(r.calls.at(-1)).toEqual({ name: 'attach', args: ['image', '/tmp/Mon dossier/é.png'] });
	});
});

describe('submit from /skill', () => {
	const submit: Command = {
		type: 'submit',
		prompt: { blocks: [{ kind: 'text', text: 'relis ce fichier' }], skills: ['review'] }
	};

	it('checks attachments, then opens a session, marks it and sends', async () => {
		const r = await run({ action: engine(submit), attachment: { blocks: [] } });
		expect(r.names()).toEqual(['parse', 'attachmentBlocks', 'ensureSession', 'mark', 'send']);
		expect(r.outcome).toEqual({
			kind: 'sent',
			sessionId: 'new-session',
			sentAt: 1000,
			runsBefore: 2,
			watch: { kind: 'skill', name: 'review' }
		});
	});

	it('labels the message with the command as typed, without the request', async () => {
		const r = await run({ action: engine(submit), session: 'sess-1' }, '/skill review relis ce fichier');
		expect(r.calls.find((c) => c.name === 'send')?.args[2]).toBe('/skill review');
	});

	it('appends the draft attachments to the prompt blocks', async () => {
		const attached: PromptBlock[] = [{ kind: 'file', path: '/p/a b.txt' }];
		const r = await run({ action: engine(submit), session: 'sess-1', attachment: { blocks: attached } });
		const sent = r.calls.find((c) => c.name === 'send');
		expect(sent?.args[1]).toEqual({
			type: 'submit',
			prompt: {
				blocks: [{ kind: 'text', text: 'relis ce fichier' }, { kind: 'file', path: '/p/a b.txt' }],
				skills: ['review']
			}
		});
	});

	it('sends nothing when an attachment cannot be sent', async () => {
		const r = await run({ action: engine(submit), attachment: { error: 'Retirez les pièces jointes en erreur.' } });
		expect(r.outcome).toEqual({ kind: 'refused', message: 'Retirez les pièces jointes en erreur.' });
		expect(r.names()).toEqual(['parse', 'attachmentBlocks']);
	});

	it('keeps the engine refusal of the send', async () => {
		const r = await run({ action: engine(submit), session: 'sess-1', sendError: 'Une génération est en cours.' });
		expect(r.outcome).toEqual({ kind: 'refused', message: 'Une génération est en cours.' });
	});
});

describe('commands that open a view, then send', () => {
	it('/search opens the search view with the query, then sends it', async () => {
		const r = await run({ action: engine({ type: 'search', text: 'todo', regex: false }), session: 'sess-1' });
		expect(r.names()).toEqual(['parse', 'open', 'mark', 'send']);
		expect(r.calls.find((c) => c.name === 'open')?.args).toEqual(['search', { query: 'todo', regex: false }]);
		expect(r.outcome.kind).toBe('sent');
	});

	it('/search without an open conversation is refused and creates no session', async () => {
		const r = await run({ action: engine({ type: 'search', text: 'todo', regex: false }) });
		expect(r.outcome).toEqual({ kind: 'refused', message: 'Aucune conversation ouverte.' });
		expect(r.names()).toEqual(['parse']);
	});

	it('/search re:<regex> keeps the regex flag', async () => {
		const r = await run({ action: engine({ type: 'search', text: 'fo+', regex: true }), session: 'sess-1' });
		expect(r.calls.find((c) => c.name === 'open')?.args).toEqual(['search', { query: 'fo+', regex: true }]);
	});

	it('/skills opens the skills view and sends the listing', async () => {
		const r = await run({ action: engine({ type: 'list_skills', query: 'rev', page: 0 }), session: 'sess-1' });
		expect(r.calls.find((c) => c.name === 'open')?.args).toEqual(['skills', { query: 'rev' }]);
		expect(r.names().at(-1)).toBe('send');
	});

	it('does not open the view and creates no session when none is open', async () => {
		const r = await run({ action: engine({ type: 'reload_skills' }) });
		expect(r.outcome).toEqual({ kind: 'refused', message: 'Aucune conversation ouverte.' });
		expect(r.names()).toEqual(['parse']);
	});
});

describe('agent_control', () => {
	it('/changes <id> apply opens the confirmation view and sends nothing', async () => {
		const r = await run({
			session: 'sess-1',
			action: engine({ type: 'agent_control', action: 'apply_changes', changeset_id: 'cs-1' })
		});
		expect(r.calls.at(-1)).toEqual({ name: 'open', args: ['changeset', { changesetId: 'cs-1', action: 'apply' }] });
		expect(r.names()).not.toContain('send');
		expect(r.outcome).toEqual({ kind: 'done' });
	});

	it('/changes without an open conversation is refused before opening anything', async () => {
		const r = await run({
			action: engine({ type: 'agent_control', action: 'inspect_changes', changeset_id: 'cs-1' })
		});
		expect(r.outcome.kind).toBe('refused');
		expect(r.names()).toEqual(['parse']);
	});

	it('/agents status opens the sub-agent inspector, then sends the status command', async () => {
		const r = await run({
			session: 'sess-1',
			action: engine({ type: 'agent_control', action: 'status', agent_id: 'ag-1' })
		});
		expect(r.names()).toEqual(['parse', 'openAgent', 'mark', 'send']);
		expect(r.calls[1]).toEqual({ name: 'openAgent', args: ['ag-1'] });
		expect(r.calls.some((c) => c.name === 'open')).toBe(false);
	});

	it('/jobs stop opens the jobs view, then sends the stop', async () => {
		const r = await run({
			session: 'sess-1',
			action: engine({ type: 'agent_control', action: 'stop_job', job_id: 'j-1' })
		});
		expect(r.calls[1]).toEqual({ name: 'open', args: ['jobs', { action: 'stop_job', jobId: 'j-1' }] });
		expect(r.outcome.kind).toBe('sent');
	});
});

describe('/clear and /compact', () => {
	it('/clear opens the explanation view and sends nothing', async () => {
		const r = await run({ session: 'sess-1', action: engine({ type: 'clear_context' }) });
		expect(r.calls).toEqual([
			{ name: 'parse', args: ['/x'] },
			{ name: 'open', args: ['clear', undefined] }
		]);
		expect(r.outcome).toEqual({ kind: 'done' });
	});

	it('/clear without a conversation explains and does nothing', async () => {
		const r = await run({ action: engine({ type: 'clear_context' }) });
		expect(r.outcome.kind).toBe('refused');
		expect(r.names()).toEqual(['parse']);
	});

	it('/compact sends and watches for its result', async () => {
		const r = await run({ session: 'sess-1', action: engine({ type: 'compact' }) });
		expect(r.outcome).toEqual({
			kind: 'sent',
			sessionId: 'sess-1',
			sentAt: 1000,
			runsBefore: 2,
			watch: { kind: 'compact' }
		});
	});

	it('/compact without a conversation is refused, not sent', async () => {
		const r = await run({ action: engine({ type: 'compact' }) });
		expect(r.outcome.kind).toBe('refused');
		expect(r.names()).toEqual(['parse']);
	});

	it('keeps the engine refusal of /compact (for example during a run)', async () => {
		const r = await run({
			session: 'sess-1',
			action: engine({ type: 'compact' }),
			sendError: 'a compaction cannot happen during a run'
		});
		expect(r.outcome).toEqual({ kind: 'refused', message: 'a compaction cannot happen during a run' });
	});
});

describe('/model and /effort without a conversation', () => {
	it('keeps the model choice locally, without an engine call', async () => {
		const r = await run({
			action: engine({ type: 'set_model', model: 'anthropic/claude-x', reasoning: 'high' })
		});
		expect(r.calls.at(-1)).toEqual({ name: 'setModel', args: ['anthropic/claude-x', 'high'] });
		expect(r.names()).not.toContain('send');
		expect(r.outcome).toEqual({ kind: 'done' });
	});

	it('shows the refusal of an unknown model', async () => {
		const r = await run({
			action: engine({ type: 'set_model', model: 'x/y' }),
			setModelError: 'Modèle inconnu : x/y.'
		});
		expect(r.outcome).toEqual({ kind: 'refused', message: 'Modèle inconnu : x/y.' });
	});

	it('/effort default clears the effort locally', async () => {
		const r = await run({ action: engine({ type: 'set_reasoning', profile: null }) });
		expect(r.calls.at(-1)).toEqual({ name: 'setEffort', args: [null] });
	});

	it('sends the model change to the engine when a conversation is open', async () => {
		const r = await run({
			session: 'sess-1',
			action: engine({ type: 'set_model', model: 'anthropic/claude-x' })
		});
		expect(r.names()).toEqual(['parse', 'mark', 'send']);
		expect(r.calls.find((c) => c.name === 'send')?.args[0]).toBe('sess-1');
	});
});
