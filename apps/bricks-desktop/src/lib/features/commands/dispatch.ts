// Runs one `/` line: the engine parses it (`slash_parse`), then each action
// is handled here. A `/` line is never sent to the model as a prompt. The
// desktop side effects go through a `SlashHost`, so the mapping is unit-tested
// with a fake host.

import type { SlashAction } from '$lib/ipc/bindings/SlashAction'
import type { SlashPresent } from '$lib/ipc/bindings/SlashPresent'
import type { Command, PromptBlock } from '$lib/ipc/engine'
import type { InspectorState, InspectorView } from '$lib/state/ui.svelte'

export type AttachKind = 'file' | 'folder' | 'image'

/** What the composer must do once a `/` line has been handled. */
export type SlashWatch =
  | { kind: 'none' }
  /** `/compact`: progress and result are shown near the composer. */
  | { kind: 'compact' }
  /** A submit: the draft stays until the run starts, or until the engine refuses it. */
  | { kind: 'submit' }
  /** `/skill`: like a submit, and the skill's activation is also watched. */
  | { kind: 'skill'; name: string }

export type SlashOutcome =
  /** Handled: the typed line is cleared (attachments stay). */
  | { kind: 'done' }
  /** Not done: the line stays in the draft and `message` is shown near the composer. */
  | { kind: 'refused'; message: string }
  /** Accepted by the engine, whose answer arrives later as events. */
  | {
      kind: 'sent'
      sessionId: string
      sentAt: number
      runsBefore: number
      watch: SlashWatch
    }

/** What the dispatcher needs from the desktop. Errors are returned as French messages. */
export interface SlashHost {
  /** The engine parser; rejects with an `Error` whose message is French. */
  parse(line: string): Promise<SlashAction>
  /** Session of the open conversation, or `null`. */
  currentSession(): string | null
  /** A session for a submit only: the open conversation, or a new one (single creation). */
  ensureSession(): Promise<{ sessionId: string } | { error: string }>
  /** Engine counters taken right before a command is sent. */
  mark(sessionId: string): { sentAt: number; runsBefore: number }
  /**
   * Sends an engine command: `null` when accepted, else the refusal. `display`
   * is the typed command of a `/skill` submit (`/skill debug`), else `null`.
   */
  send(sessionId: string, command: Command, display?: string | null): Promise<string | null>
  /** Model choice when no conversation is open (kept for the first send). */
  setModel(selection: string, reasoning: string | null): Promise<string | null>
  /** Effort choice when no conversation is open. */
  setEffort(profile: string | null): Promise<string | null>
  openPicker(picker: 'model' | 'effort'): void
  open(view: InspectorView, args?: InspectorState['args']): void
  /** Attachments of the draft as prompt blocks, or the reason they cannot be sent. */
  attachmentBlocks(): { blocks: PromptBlock[] } | { error: string }
  resume(sessionId: string): Promise<string | null>
  showChanges(): void
  attach(kind: AttachKind, path: string): Promise<string | null>
  quit(): Promise<string | null>
}

const DONE: SlashOutcome = { kind: 'done' }
const NO_WATCH: SlashWatch = { kind: 'none' }
const NO_CONVERSATION = 'Aucune conversation ouverte.'

function refused(message: string): SlashOutcome {
  return { kind: 'refused', message }
}

function fromError(error: string | null): SlashOutcome {
  return error === null ? DONE : refused(error)
}

/** The `/skill <name>` part of a typed line, as typed: the label of its message. */
function skillLabel(line: string): string | null {
  const match = /^(\S+)\s+(\S+)/.exec(line)
  return match ? `${match[1]} ${match[2]}` : null
}

/** Parses and runs one `/` line. */
export async function runSlashLine(line: string, host: SlashHost): Promise<SlashOutcome> {
  let action: SlashAction
  try {
    action = await host.parse(line)
  } catch (e) {
    return refused(e instanceof Error ? e.message : 'Commande invalide.')
  }
  return action.kind === 'present'
    ? runPresent(action.present, host)
    : runEngine(action.command, host, line.trim())
}

async function runPresent(p: SlashPresent, host: SlashHost): Promise<SlashOutcome> {
  switch (p.view) {
    case 'model_picker':
      host.openPicker('model')
      return DONE
    case 'effort_picker':
      host.openPicker('effort')
      return DONE
    case 'memory':
      host.open('memory')
      return DONE
    case 'context':
      host.open('context')
      return DONE
    case 'context_details':
      host.open('context', { details: true })
      return DONE
    case 'cost':
      host.open('cost')
      return DONE
    case 'cost_details':
      host.open('cost', { details: true })
      return DONE
    case 'session':
      host.open('session')
      return DONE
    case 'session_picker':
      host.open('sessions')
      return DONE
    case 'resume':
      return fromError(await host.resume(p.session_id))
    case 'diff':
      host.showChanges()
      return DONE
    case 'tools':
      host.open('tools')
      return DONE
    case 'mcp':
      host.open('mcp')
      return DONE
    case 'config':
      host.open('config')
      return DONE
    case 'help':
      host.open('help')
      return DONE
    case 'quit':
      return fromError(await host.quit())
    case 'attach': {
      const kind = p.kind === 'file' || p.kind === 'folder' || p.kind === 'image' ? p.kind : null
      if (kind === null) return refused(`Type de pièce jointe inconnu : ${p.kind}.`)
      return fromError(await host.attach(kind, p.path))
    }
    default: {
      const unknown: never = p
      return refused(`Commande inconnue : ${JSON.stringify(unknown)}`)
    }
  }
}

/** Sends a command to a known session and reports what the composer must watch. */
async function sendTo(
  host: SlashHost,
  sessionId: string,
  command: Command,
  watch: SlashWatch,
  display: string | null = null,
): Promise<SlashOutcome> {
  const { sentAt, runsBefore } = host.mark(sessionId)
  const error = await host.send(sessionId, command, display)
  if (error !== null) return refused(error)
  return { kind: 'sent', sessionId, sentAt, runsBefore, watch }
}

/**
 * Opens a view, then sends the command that fills it (the answer comes as
 * events). Needs the open conversation: without one, nothing is created.
 */
async function openAndSend(
  host: SlashHost,
  view: InspectorView,
  args: InspectorState['args'],
  command: Command,
): Promise<SlashOutcome> {
  const sid = host.currentSession()
  if (sid === null) return refused(NO_CONVERSATION)
  host.open(view, args)
  return sendTo(host, sid, command, NO_WATCH)
}

const CHANGESET_ACTIONS: Record<string, 'inspect' | 'apply' | 'discard'> = {
  inspect_changes: 'inspect',
  apply_changes: 'apply',
  discard_changes: 'discard',
}

async function runEngine(command: Command, host: SlashHost, line: string): Promise<SlashOutcome> {
  switch (command.type) {
    case 'set_model': {
      if (!command.model) {
        host.openPicker('model')
        return DONE
      }
      const sid = host.currentSession()
      if (sid === null) {
        return fromError(await host.setModel(command.model, command.reasoning ?? null))
      }
      return sendTo(host, sid, command, NO_WATCH)
    }
    case 'set_reasoning': {
      const sid = host.currentSession()
      if (sid === null) return fromError(await host.setEffort(command.profile))
      return sendTo(host, sid, command, NO_WATCH)
    }
    case 'submit': {
      // Attachments are checked first: a blocked draft must not create a session.
      const parts = host.attachmentBlocks()
      if ('error' in parts) return refused(parts.error)
      const session = await host.ensureSession()
      if ('error' in session) return refused(session.error)
      const skill = command.prompt.skills?.[0]
      const prompt = { ...command.prompt, blocks: [...command.prompt.blocks, ...parts.blocks] }
      return sendTo(
        host,
        session.sessionId,
        { type: 'submit', prompt },
        skill ? { kind: 'skill', name: skill } : { kind: 'submit' },
        skill ? skillLabel(line) : null,
      )
    }
    case 'cancel': {
      const sid = host.currentSession()
      if (sid === null) return refused('Aucune génération en cours.')
      return sendTo(host, sid, command, NO_WATCH)
    }
    case 'compact': {
      const sid = host.currentSession()
      if (sid === null) return refused('Ouvrez une conversation pour compacter son contexte.')
      return sendTo(host, sid, command, { kind: 'compact' })
    }
    case 'clear_context': {
      if (host.currentSession() === null) {
        return refused('Ouvrez une conversation pour vider son contexte.')
      }
      // The view explains the scope and asks for the confirmation; nothing is sent here.
      host.open('clear')
      return DONE
    }
    case 'search':
      return openAndSend(
        host,
        'search',
        { query: command.text, regex: command.regex ?? false },
        command,
      )
    case 'list_agent_profiles':
      return openAndSend(host, 'agents', { query: command.query ?? '' }, command)
    case 'reload_agent_profiles':
      return openAndSend(host, 'agents', {}, command)
    case 'list_skills':
      return openAndSend(host, 'skills', { query: command.query ?? '' }, command)
    case 'reload_skills':
    case 'trust_project_skills':
      return openAndSend(host, 'skills', {}, command)
    case 'agent_control':
      return agentControl(command, host)
    case 'resume':
      return fromError(await host.resume(command.session_id))
    case 'approve':
      return refused('Les demandes d’approbation se traitent dans la conversation.')
    default: {
      const unknown: never = command
      return refused(`Commande inconnue : ${JSON.stringify(unknown)}`)
    }
  }
}

async function agentControl(
  command: Extract<Command, { type: 'agent_control' }>,
  host: SlashHost,
): Promise<SlashOutcome> {
  const { action } = command
  const sid = host.currentSession()
  const changeAction = CHANGESET_ACTIONS[action]
  if (changeAction) {
    if (sid === null) return refused('Ouvrez une conversation pour voir ses modifications isolées.')
    // Inspect, apply and discard are confirmed in the view: nothing is sent here.
    host.open('changeset', { changesetId: command.changeset_id, action: changeAction })
    return DONE
  }
  if (sid === null) return refused('Ouvrez une conversation pour utiliser cette commande.')
  if (action === 'jobs' || action === 'stop_job') {
    host.open('jobs', { action, jobId: command.job_id })
  } else {
    host.open('agent_instances', { action, agentId: command.agent_id })
  }
  return sendTo(host, sid, command, NO_WATCH)
}
