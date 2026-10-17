// Engine requests sent by the catalog views, and the answers they wait for.
//
// The engine answers a command later, as an event that replaces one field of
// the conversation with a new object. A request is answered once that field
// holds another object than when the command was sent (and, for
// `agent_control`, once the answer carries the expected action). A refusal
// comes either from the IPC call (`app.command` returns the message) or as a
// `command_rejected` event of the same command name. Nothing is faked: while
// neither has happened, the request is pending.

import type { Command } from '$lib/ipc/engine';
import { app } from '$lib/state/app.svelte';
import type { Conversation } from '$lib/state/conversation.svelte';

export type ProfilesAnswer = NonNullable<Conversation['agentProfiles']>;
export type SkillsAnswer = NonNullable<Conversation['skillsListing']>;
export type ControlAnswer = NonNullable<Conversation['lastAgentControl']>;

/** The conversation fields the engine's answers replace. */
export const profilesSlot = (conv: Conversation) => conv.agentProfiles;
export const skillsSlot = (conv: Conversation) => conv.skillsListing;
export const controlSlot = (conv: Conversation) => conv.lastAgentControl;

interface Sent {
	conv: Conversation;
	/** Command name, as `command_rejected` names it. */
	command: string;
	/** Action the `agent_control` answer must carry, when there is one. */
	action: string | null;
	slot: (conv: Conversation) => unknown;
	/** The field as it was when the command was sent. */
	seen: unknown;
	/** The last rejection as it was when the command was sent. */
	rejection: unknown;
}

export class EngineRequest {
	#sent = $state.raw<Sent | null>(null);
	#refused = $state<string | null>(null);

	/**
	 * Sends `command` to the open conversation. `slot` reads the field the
	 * answer replaces; `action` is the `agent_control` action the answer must
	 * carry. Does nothing when no conversation is open.
	 */
	async send(
		command: Command,
		slot: (conv: Conversation) => unknown,
		action: string | null = null
	): Promise<void> {
		const conv = app.current;
		if (!conv) return;
		const sent: Sent = {
			conv,
			command: command.type,
			action,
			slot,
			seen: slot(conv),
			rejection: conv.lastRejection
		};
		this.#sent = sent;
		this.#refused = null;
		const error = await app.command(conv.sessionId, command);
		// A refused command gets no answer: show the refusal, stop waiting.
		if (error && this.#sent === sent) {
			this.#refused = error;
			this.#sent = null;
		}
	}

	/** The answer, once it has arrived; otherwise null. */
	answered<T>(): T | null {
		const sent = this.#sent;
		if (!sent) return null;
		const value = sent.slot(sent.conv);
		if (value === null || value === undefined || value === sent.seen) return null;
		if (sent.action !== null && (value as { action?: unknown }).action !== sent.action) {
			return null;
		}
		return value as T;
	}

	/** Sent, not refused, not answered yet. */
	get pending(): boolean {
		return this.#sent !== null && this.refusal === null && this.answered() === null;
	}

	/** The IPC call or the engine refused the command. */
	get refusal(): string | null {
		if (this.#refused) return this.#refused;
		const sent = this.#sent;
		if (!sent) return null;
		const last = sent.conv.lastRejection;
		return last && last !== sent.rejection && last.command === sent.command ? last.reason : null;
	}
}
