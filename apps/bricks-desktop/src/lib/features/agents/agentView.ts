// Sub-agent presentation: state mapping, durations and French labels.
// Pure helpers, no component state.

import { t, numberFormat, dateTimeFormat } from '$lib/i18n/i18n.svelte';
import type { ChangeSetState, InstanceState, Termination } from '$lib/ipc/engine';
import { TERMINAL_AGENT_STATES } from '$lib/ipc/engine';
import type { AgentInfo } from '$lib/state/conversation.svelte';
import type { StatusState } from '$lib/layout/StatusDot.svelte';
import type { BadgeVariant } from '$lib/components/ui/badge';

export interface AgentStateView {
	dot: StatusState;
	label: string;
}

const STATE_VIEW: Record<InstanceState, AgentStateView> = {
	created: { dot: 'waiting', get label() { return t('common.created') } },
	queued: { dot: 'waiting', get label() { return t('agents.queued') } },
	waiting_admission: { dot: 'waiting', get label() { return t('agents.waitingForAdmission') } },
	starting: { dot: 'running', get label() { return t('agents.starting') } },
	running: { dot: 'running', get label() { return t('common.running') } },
	cancelling: { dot: 'running', get label() { return t('agents.cancelling') } },
	completed: { dot: 'success', get label() { return t('common.finished') } },
	incomplete: { dot: 'waiting', get label() { return t('common.incomplete') } },
	failed: { dot: 'error', get label() { return t('common.failed') } },
	interrupted: { dot: 'error', get label() { return t('common.interrupted') } },
	cancelled: { dot: 'cancelled', get label() { return t('common.cancelled') } }
};

export function agentStateView(state: InstanceState): AgentStateView {
	return STATE_VIEW[state];
}

/** Not yet finished: the engine may still change this agent. */
export function isLive(state: InstanceState): boolean {
	return !TERMINAL_AGENT_STATES.includes(state);
}

/** The person may ask the engine to stop this agent. */
export function canCancel(state: InstanceState): boolean {
	return isLive(state) && state !== 'cancelling';
}

export function agentDurationMs(agent: AgentInfo, now: number): number {
	return Math.max(0, (agent.endedAt ?? now) - agent.spawnedAt);
}

export { formatDuration } from '../conversation/format';

function baseName(path: unknown): string | null {
	if (typeof path !== 'string' || path.trim() === '') return null;
	return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

function field(input: unknown, ...keys: string[]): unknown {
	if (typeof input !== 'object' || input === null) return undefined;
	const record = input as Record<string, unknown>;
	for (const key of keys) if (record[key] !== undefined) return record[key];
	return undefined;
}

/** What one running tool call is doing, from its name and input only. */
function toolAction(name: string, input: unknown): string {
	const file = baseName(field(input, 'file_path', 'path'));
	switch (name) {
		case 'Read':
			return file ? t('agents.reading', { file }) : t('common.reading');
		case 'Write':
		case 'Edit':
		case 'ApplyPatch':
			return file ? t('agents.editing', { file }) : t('agents.editing2');
		case 'Glob':
		case 'Grep':
		case 'Search':
			return t('common.codeSearch');
		case 'Bash': {
			const command = field(input, 'command');
			return typeof command === 'string' && command.trim() !== '' ? t('agents.command', { command: command.trim() }) : t('agents.command2');
		}
		case 'WebFetch':
		case 'WebSearch':
			return t('agents.webSearch');
		case 'Agent':
			return t('agents.delegatingToASubAgent');
		case 'Messagent':
			return t('agents.messageToTheTeam');
		case 'AgentControl':
			return t('agents.followingItsSubAgents');
		default:
			return name;
	}
}

/**
 * The second line of an agent in the thread: what it is doing now, from the
 * tool calls the engine reported as running (nothing is guessed or counted
 * beyond them), or what it waits for. `null` when there is nothing to say.
 */
export function agentAction(agent: AgentInfo, awaitingApproval: boolean): string | null {
	if (!isLive(agent.state)) return null;
	if (awaitingApproval) return t('common.waitingForApproval');
	if (agent.state === 'queued' || agent.state === 'waiting_admission' || agent.state === 'created') {
		return agent.reason === 'preparing its worktree' ? t('agents.preparingItsWorkspace') : null;
	}
	const running = agent.tools.filter((t) => t.status === 'running');
	if (running.length === 0) return null;
	if (running.length === 1) return `${toolAction(running[0].name, running[0].input)}…`;
	const names = new Set(running.map((t) => t.name));
	if (names.size === 1 && (names.has('Read') || names.has('Edit') || names.has('Write'))) {
		const verb = names.has('Read') ? t('common.reading') : t('agents.editing2');
		return t('agents.files', { verb, running: running.length });
	}
	return t('agents.toolsRunning', { running: running.length });
}

const TIME = dateTimeFormat({
	hour: '2-digit',
	minute: '2-digit',
	second: '2-digit'
});

export function formatTime(ms: number): string {
	return TIME.format(new Date(ms));
}

const COUNT = numberFormat();
const USD = numberFormat({
	style: 'currency',
	currency: 'USD',
	maximumFractionDigits: 4
});

export function formatUsd(amount: number): string {
	return USD.format(amount);
}

export function formatCount(n: number): string {
	return COUNT.format(n);
}

/** Heading of a successful engine answer to a control action. */
export function controlDoneLabel(action: string): string {
	switch (action) {
		case 'cancel':
			return t('agents.stopRequested');
		case 'apply_changes':
			return t('agents.changesApplied');
		case 'discard_changes':
			return t('agents.changesDiscarded');
		default:
			return t('agents.actionCompleted');
	}
}

/** Heading of a failed engine answer; the engine's own text follows it. */
export function controlFailedLabel(action: string): string {
	switch (action) {
		case 'cancel':
			return t('agents.couldNotStop');
		case 'apply_changes':
			return t('common.couldNotApply');
		case 'discard_changes':
			return t('common.couldNotDiscard');
		default:
			return t('agents.actionFailed');
	}
}

export function isolationLabel(isolation: string): string {
	return isolation === 'worktree' ? t('agents.isolatedWorktree') : t('agents.sharedWorkspace');
}

export function terminationLabel(end: Termination | undefined): string | null {
	if (!end) return null;
	switch (end.kind) {
		case 'completed':
			return t('agents.finishedNormally');
		case 'output_truncated':
			return t('agents.outputTruncatedContinuation', { count: String(end.continuations) });
		case 'no_progress':
			return t('agents.noProgressRepetition', { count: String(end.repeats) });
		default:
			// A kind of an earlier version: interrupted, never a success.
			return t('common.interruptedEndOldSession');
	}
}

/** Git porcelain letter of a changed file. */
export function changeStatusLabel(code: string): string {
	switch (code) {
		case 'A':
			return t('common.added');
		case 'M':
			return t('common.modified');
		case 'D':
			return t('common.deleted');
		case 'R':
			return t('common.renamed');
		case 'C':
			return t('common.copied');
		default:
			return code;
	}
}

export function changesetLabel(state: ChangeSetState): string {
	switch (state) {
		case 'ready':
			return t('agents.readyToApply');
		case 'applied':
			return t('agents.applied');
		case 'conflict':
			return t('common.conflict');
		case 'discarded':
			return t('agents.discarded');
	}
}

export function changesetVariant(state: ChangeSetState): BadgeVariant {
	switch (state) {
		case 'ready':
			return 'default';
		case 'applied':
			return 'secondary';
		case 'conflict':
			return 'destructive';
		case 'discarded':
			return 'outline';
	}
}
