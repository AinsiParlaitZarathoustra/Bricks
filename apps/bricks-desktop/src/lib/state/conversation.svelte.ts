// Per-session state, built from the stored history and the engine's
// envelopes. One instance per open session, kept while the user looks
// at other conversations so no event is lost. Owner: orchestrator.

import { SvelteMap } from 'svelte/reactivity'
import type { HistoryMessage, OpenedSession, SessionItem } from '$lib/ipc/client'
import type {
  InstanceRecord,
  AgentProfilesEvent,
  SearchHit,
  SkillsListing,
  Usage,
  AgentResult,
  ApprovalRequest,
  AttachmentInfo,
  ChangeSet,
  ContextStatus,
  DecidedBy,
  Decision,
  Envelope,
  InstanceState,
  RunOutcome,
  SkillSourceInfo,
  Snapshot,
  SpawnInfo,
  Termination,
  WrittenFile,
} from '$lib/ipc/engine'
import { TERMINAL_AGENT_STATES } from '$lib/ipc/engine'

/** Deeply reactive copy, for objects stored in a SvelteMap and mutated in place. */
function reactive<T extends object>(value: T): T {
  const proxy = $state(value)
  return proxy
}

let nextLocalId = 0
const localId = (prefix: string) => `${prefix}-${++nextLocalId}`

export interface UserItem {
  kind: 'user'
  id: string
  runId?: string
  text: string
  attachments: AttachmentInfo[]
  at?: number
}

export interface AssistantItem {
  kind: 'assistant'
  id: string
  runId?: string
  text: string
  /** Reasoning the provider exposed (never reconstructed). */
  thinking: string
  streaming: boolean
}

export type ToolStatus = 'running' | 'ok' | 'error'

export interface ToolItem {
  kind: 'tool'
  id: string
  toolCallId: string
  runId?: string
  name: string
  input: unknown
  status: ToolStatus
  output?: string
  durationMs?: number
  progress?: string
}

export interface ApprovalItem {
  kind: 'approval'
  id: string
  runId?: string
  approval: ApprovalRequest
  resolved?: { decision: Decision; by: DecidedBy }
}

export interface EditItem {
  kind: 'edit'
  id: string
  runId?: string
  toolCallId: string
  tool: string
  files: WrittenFile[]
  agentId?: string
  changesetId?: string
}

export interface NoticeItem {
  kind: 'notice'
  id: string
  runId?: string
  level: 'info' | 'warning' | 'error'
  message: string
}

export interface RunEndItem {
  kind: 'run_end'
  id: string
  runId: string
}

export type TimelineItem =
  | UserItem
  | AssistantItem
  | ToolItem
  | ApprovalItem
  | EditItem
  | NoticeItem
  | RunEndItem

/** Line sums of applied edits for one path (operations, not a net diff). */
export interface FileTotals {
  path: string
  added: number
  removed: number
  binary: boolean
  /** Number of edit operations summed. */
  operations: number
}

export interface CommandEvidence {
  toolCallId: string
  name: string
  command: string
  isError: boolean
  durationMs: number
  output: string
}

export interface RunInfo {
  runId: string
  prompt: string
  model: string
  reasoning?: string
  startedAt: number
  finishedAt?: number
  outcome?: RunOutcome
  error?: string
  termination?: Termination
  finalText?: string
  turns?: number
  approvalsUnsatisfied: ApprovalRequest[]
  /** Edit operations of this run, keyed by path. */
  files: SvelteMap<string, FileTotals>
  /** Shell commands the run executed, with their real output. */
  commands: CommandEvidence[]
}

export interface AgentToolCall {
  toolCallId: string
  name: string
  input: unknown
  status: ToolStatus
  durationMs?: number
}

export interface AgentInfo {
  id: string
  spawn: SpawnInfo
  state: InstanceState
  reason?: string
  spawnedAt: number
  endedAt?: number
  tools: AgentToolCall[]
  result?: AgentResult
}

export interface DraftAttachment {
  id: string
  /** `file` | `folder` | `image`. */
  kind: 'file' | 'folder' | 'image'
  path: string
  name: string
  /** Object URL / data URL for an image preview. */
  previewUrl?: string
  error?: string
}

export interface Draft {
  text: string
  attachments: DraftAttachment[]
  /** Skills to activate with the next message (`Prompt.skills`), Sprint 14.3. */
  skills: string[]
}

export interface TurnUsage {
  at: number
  runId?: string
  usage: Usage
}

export interface JobInfo {
  jobId: string
  agentId: string
  rootRunId: string
  command: string
  cwd: string
  pid: number
  startedAt: number
  stdoutBytes: number
  stderrBytes: number
  /** `running` until `job_finished`: then `completed` | `failed` | `stopped`. */
  state: string
  code?: number
  signal?: number
  durationMs?: number
  logs: string[]
}

export interface SearchResult {
  at: number
  query: string
  status: string
  hits: SearchHit[]
  omitted: number
  notes: string[]
  elapsedMs: number
}

export interface MemoryState {
  lastRecall?: { at: number; items: number; tokens: number; omitted: number; budget: number }
  maintenanceRunning: boolean
  lastMaintenance?: { at: number; outcome: string; report?: unknown; error?: string }
}

const SHELL_TOOLS = new Set(['Bash', 'bash', 'Shell', 'PowerShell'])

export class Conversation {
  readonly sessionId: string
  projectId: string | null

  session = $state<SessionItem>() as SessionItem
  title = $state('')
  workingDir = $state('')
  model = $state('')
  reasoning = $state<string | null>(null)
  effectiveReasoning = $state<string | null>(null)
  /** Set when a model/reasoning change waits for the next turn/run. */
  pendingChange = $state<{ what: string; applies: string } | null>(null)

  items = $state<TimelineItem[]>([])
  runs = new SvelteMap<string, RunInfo>()
  agents = new SvelteMap<string, AgentInfo>()
  changesets = new SvelteMap<string, ChangeSet>()
  approvals = new SvelteMap<string, ApprovalRequest>()
  skillsTrust = $state<{ project: string; sources: SkillSourceInfo[] } | null>(null)

  /** Active run id (`null` when idle). */
  runningRunId = $state<string | null>(null)
  busy = $state<string | null>(null)

  context = $state<ContextStatus | null>(null)
  /** The last measure no longer applies (model change, compaction...). */
  contextStale = $state(false)
  usageTotal = $state<{ input: number; output: number; costUsd?: number } | null>(null)

  draft = $state<Draft>({ text: '', attachments: [], skills: [] })
  /** Last refusal of a command (e.g. submit during a run). */
  lastRejection = $state<{ command: string; reason: string; at: number } | null>(null)
  /** Last answer to an `agent_control` command (apply/discard/cancel). */
  lastAgentControl = $state<{ action: string; ok: boolean; text: string; at: number } | null>(null)

  // Sprint 14.1: answers of the migrated commands, kept per session.
  /** Per-response usage, in order (`usage.turn`); session total is `usageTotalFull`. */
  turnUsages = $state<TurnUsage[]>([])
  usageTotalFull = $state<Usage | null>(null)
  /** Last `run_usage` of a root run (own + descendants, each response once). */
  runUsages = new SvelteMap<string, Extract<Envelope, { type: 'run_usage' }>>()
  jobs = new SvelteMap<string, JobInfo>()
  searchResult = $state<SearchResult | null>(null)
  agentProfiles = $state<AgentProfilesEvent | null>(null)
  skillsListing = $state<{ at: number; listing: SkillsListing } | null>(null)
  memory = $state<MemoryState>({ maintenanceRunning: false })
  /** Last compaction answer (`compaction` event). */
  /** Last `context_cleared` answer (`/clear`). */
  lastClear = $state<{ at: number; messagesRemoved: number } | null>(null)
  lastCompaction = $state<{ at: number; reason: string; outcome: string; compacted: boolean } | null>(null)
  /** Last skill activation outcome (`/skill`). */
  lastSkill = $state<{ at: number; name: string; ok: boolean; reason?: string } | null>(null)

  lastSeq = -1

  constructor(opened: OpenedSession, projectId: string | null) {
    this.sessionId = opened.session.id
    this.projectId = projectId
    this.session = opened.session
    this.title = opened.session.title
    this.applySnapshot(opened.snapshot)
    this.loadHistory(opened.history)
    this.replay(opened.recent_events)
  }

  get running(): boolean {
    return this.runningRunId !== null
  }

  get pendingApprovals(): ApprovalRequest[] {
    return [...this.approvals.values()]
  }

  /** Run of a timeline item. */
  run(runId: string | undefined): RunInfo | undefined {
    return runId ? this.runs.get(runId) : undefined
  }

  /**
   * Restores sub-agents and ChangeSets kept by the engine runtime (after a
   * reopen the events of earlier runs are gone). Live entries win.
   */
  hydrateAgents(records: InstanceRecord[], changesets: ChangeSet[]) {
    for (const r of records) {
      if (this.agents.has(r.info.agent_id)) continue
      this.agents.set(
        r.info.agent_id,
        reactive<AgentInfo>({
          id: r.info.agent_id,
          spawn: r.info,
          state: r.state,
          reason: r.reason,
          spawnedAt: Date.parse(r.info.created_at) || r.updated_at_ms,
          endedAt: TERMINAL_AGENT_STATES.includes(r.state) ? r.updated_at_ms : undefined,
          tools: [],
          result: r.result,
        }),
      )
    }
    for (const cs of changesets) if (!this.changesets.has(cs.id)) this.changesets.set(cs.id, reactive(cs))
  }

  applySnapshot(s: Snapshot) {
    this.usageTotalFull = s.usage
    this.title = s.title || this.title
    this.workingDir = s.working_dir
    this.model = s.model
    this.reasoning = s.reasoning
    this.effectiveReasoning = s.effective_reasoning
    this.context = s.context
    this.contextStale = false
    this.runningRunId = s.activity.state === 'running' ? s.activity.run_id : null
    this.busy = s.activity.state === 'busy' ? s.activity.what : null
    this.approvals.clear()
    for (const a of s.pending_approvals) this.approvals.set(a.approval_id, a)
  }

  updateSession(item: SessionItem) {
    this.session = item
    this.title = item.title
  }

  private loadHistory(history: HistoryMessage[]) {
    const items: TimelineItem[] = []
    const tools = new Map<string, ToolItem>()
    for (const msg of history) {
      if (msg.role === 'system') continue
      for (const block of msg.blocks) {
        switch (block.type) {
          case 'text':
            if (!block.text.trim()) break
            if (msg.role === 'user') {
              items.push({ kind: 'user', id: localId('u'), text: block.text, attachments: [] })
            } else {
              const last = items.at(-1)
              if (last?.kind === 'assistant') last.text += block.text
              else
                items.push({
                  kind: 'assistant',
                  id: localId('a'),
                  text: block.text,
                  thinking: '',
                  streaming: false,
                })
            }
            break
          case 'thinking': {
            const last = items.at(-1)
            if (last?.kind === 'assistant') last.thinking += block.text
            else
              items.push({
                kind: 'assistant',
                id: localId('a'),
                text: '',
                thinking: block.text,
                streaming: false,
              })
            break
          }
          case 'tool_use': {
            const t: ToolItem = {
              kind: 'tool',
              id: localId('t'),
              toolCallId: block.id,
              name: block.name,
              input: block.input,
              status: 'ok',
            }
            tools.set(block.id, t)
            items.push(t)
            break
          }
          case 'tool_result': {
            const t = tools.get(block.tool_use_id)
            if (t) {
              t.output = block.content
              t.status = block.is_error ? 'error' : 'ok'
            }
            break
          }
          case 'image': {
            const last = items.at(-1)
            if (msg.role === 'user' && block.path) {
              const info: AttachmentInfo = { kind: 'image', path: block.path, bytes: 0 }
              if (last?.kind === 'user') last.attachments.push(info)
              else
                items.push({ kind: 'user', id: localId('u'), text: '', attachments: [info] })
            }
            break
          }
        }
      }
    }
    this.items = items
  }

  /**
   * Re-applies the bridge's recent envelopes after the stored history: a
   * running run's events are replayed (its partial messages are dropped
   * from the history first, they come back from the events); for a
   * finished run only the run summary data is rebuilt (no text twice).
   */
  private replay(recent: Envelope[]) {
    if (recent.length === 0) return
    const lastRun = [...recent].reverse().find((e) => e.type === 'run_started')
    const running = this.runningRunId
    if (running && lastRun && lastRun.run_id === running) {
      const prompt = lastRun.type === 'run_started' ? lastRun.prompt : ''
      const idx = this.items.findLastIndex((i) => i.kind === 'user' && i.text === prompt)
      if (idx >= 0) this.items = this.items.slice(0, idx)
      for (const env of recent) {
        if (env.run_id === running || !env.run_id) this.apply(env)
        else this.lastSeq = Math.max(this.lastSeq, env.seq)
      }
      return
    }
    // Idle: rebuild run/agent bookkeeping and the end card, not the text.
    for (const env of recent) {
      switch (env.type) {
        case 'text_delta':
        case 'thinking_delta':
        case 'tool_started':
        case 'tool_progress':
        case 'tool_finished':
        case 'notice':
          this.lastSeq = Math.max(this.lastSeq, env.seq)
          if (env.type === 'tool_finished') this.recordCommand(env)
          break
        case 'run_started':
          this.startRun(env, false)
          break
        default:
          this.apply(env)
      }
    }
  }

  /** Applies one envelope; duplicates (seq already seen) are ignored. */
  apply(env: Envelope) {
    // Another session's envelope never touches this one (nor its seq).
    if (env.session_id !== this.sessionId) return
    if (env.seq <= this.lastSeq) return
    this.lastSeq = env.seq
    const runId = env.run_id
    switch (env.type) {
      case 'session_opened':
        this.workingDir = env.working_dir
        this.model = env.model
        this.reasoning = env.reasoning ?? null
        this.effectiveReasoning = env.effective_reasoning ?? null
        for (const w of env.warnings) this.notice('warning', w, runId)
        break
      case 'run_started':
        this.startRun(env, true)
        break
      case 'text_delta': {
        const a = this.currentAssistant(runId)
        a.text += env.text
        break
      }
      case 'thinking_delta': {
        const a = this.currentAssistant(runId)
        a.thinking += env.text
        break
      }
      case 'tool_started':
        this.closeAssistant()
        this.items.push({
          kind: 'tool',
          id: localId('t'),
          toolCallId: env.tool_call_id,
          runId,
          name: env.name,
          input: env.input,
          status: 'running',
        })
        break
      case 'tool_progress': {
        const t = this.findTool(env.tool_call_id, env.name)
        if (t) t.progress = env.message
        break
      }
      case 'tool_finished': {
        const t = this.findTool(env.tool_call_id, env.name)
        if (t) {
          t.status = env.is_error ? 'error' : 'ok'
          t.output = env.output
          t.durationMs = env.duration_ms
          t.progress = undefined
        }
        this.recordCommand(env)
        break
      }
      case 'approval_requested':
        this.closeAssistant()
        this.approvals.set(env.approval.approval_id, env.approval)
        this.items.push({ kind: 'approval', id: localId('ap'), runId, approval: env.approval })
        break
      case 'approval_resolved': {
        this.approvals.delete(env.approval_id)
        const item = this.items.findLast(
          (i): i is ApprovalItem =>
            i.kind === 'approval' && i.approval.approval_id === env.approval_id,
        )
        if (item) item.resolved = { decision: env.decision, by: env.by }
        break
      }
      case 'edit_applied': {
        this.items.push({
          kind: 'edit',
          id: localId('e'),
          runId,
          toolCallId: env.tool_call_id,
          tool: env.tool,
          files: env.files,
          agentId: env.agent_id,
          changesetId: env.changeset_id,
        })
        const run = this.run(runId)
        if (run) {
          for (const f of env.files) {
            const prev = run.files.get(f.path)
            run.files.set(f.path, {
              path: f.path,
              added: (prev?.added ?? 0) + f.added,
              removed: (prev?.removed ?? 0) + f.removed,
              binary: (prev?.binary ?? false) || !!f.binary,
              operations: (prev?.operations ?? 0) + 1,
            })
          }
        }
        break
      }
      case 'context':
        this.context = env.status
        this.contextStale = false
        break
      case 'usage':
        this.turnUsages.push({ at: env.at, runId, usage: env.turn })
        if (this.turnUsages.length > 2000) this.turnUsages.splice(0, this.turnUsages.length - 2000)
        this.usageTotalFull = env.total
        this.usageTotal = {
          input: env.total.input_tokens,
          output: env.total.output_tokens,
          costUsd: env.total.cost_usd,
        }
        break
      case 'compaction':
        this.lastCompaction = { at: env.at, reason: env.reason, outcome: env.outcome, compacted: env.compacted }
        this.contextStale = env.compacted || this.contextStale
        this.notice(
          'info',
          `Compaction (${env.reason}) : ${env.outcome}`,
          runId,
        )
        break
      case 'context_cleared':
        this.lastClear = { at: env.at, messagesRemoved: env.messages_removed }
        this.contextStale = true
        this.notice('info', `Contexte vidé (${env.messages_removed} messages retirés).`, runId)
        break
      case 'model_changed':
        this.model = env.model
        this.reasoning = env.reasoning ?? null
        this.effectiveReasoning = env.effective_reasoning ?? null
        this.contextStale = true
        this.pendingChange = this.running ? { what: 'model', applies: env.applies } : null
        break
      case 'reasoning_changed':
        this.reasoning = env.requested ?? null
        this.effectiveReasoning = env.effective ?? null
        this.pendingChange = this.running ? { what: 'reasoning', applies: env.applies } : null
        break
      case 'notice':
        this.notice('info', env.message, runId)
        break
      case 'run_finished': {
        this.closeAssistant()
        const id = runId ?? this.runningRunId
        const run = this.run(id ?? undefined)
        if (run) {
          run.finishedAt = env.at
          run.outcome = env.outcome
          run.error = env.error
          run.termination = env.termination
          run.finalText = env.text
          run.turns = env.turns
          run.approvalsUnsatisfied = env.approvals_unsatisfied ?? []
        }
        if (id) this.items.push({ kind: 'run_end', id: localId('r'), runId: id })
        if (this.runningRunId === id) this.runningRunId = null
        this.pendingChange = null
        for (const item of this.items)
          if (item.kind === 'tool' && item.status === 'running' && item.runId === id)
            item.status = 'error'
        break
      }
      case 'agent_spawned':
        this.agents.set(
          env.agent.agent_id,
          reactive<AgentInfo>({
            id: env.agent.agent_id,
            spawn: env.agent,
            state: 'created',
            spawnedAt: env.at,
            tools: [],
          }),
        )
        break
      case 'agent_state': {
        const a = this.agents.get(env.agent_id)
        if (a) {
          a.state = env.state
          a.reason = env.reason
          if (TERMINAL_AGENT_STATES.includes(env.state)) a.endedAt = env.at
        }
        break
      }
      case 'agent_tool_started': {
        const a = this.agents.get(env.agent_id)
        a?.tools.push({
          toolCallId: env.tool_call_id,
          name: env.name,
          input: env.input,
          status: 'running',
        })
        break
      }
      case 'agent_tool_finished': {
        const t = this.agents
          .get(env.agent_id)
          ?.tools.findLast((t) => t.toolCallId === env.tool_call_id)
        if (t) {
          t.status = env.is_error ? 'error' : 'ok'
          t.durationMs = env.duration_ms
        }
        break
      }
      case 'agent_finished': {
        const a = this.agents.get(env.result.agent_id)
        if (a) {
          a.result = env.result
          a.endedAt ??= env.at
        }
        break
      }
      case 'changes_ready':
        this.changesets.set(env.changeset.id, reactive(env.changeset))
        break
      case 'changes_updated': {
        const cs = this.changesets.get(env.changeset_id)
        if (cs) cs.state = env.state
        break
      }
      case 'skills_trust_requested':
        this.skillsTrust = { project: env.project, sources: env.sources }
        break
      case 'skills_trust_resolved':
        this.skillsTrust = null
        break
      case 'run_usage':
        this.runUsages.set(env.root_run_id, env)
        break
      case 'memory_recalled':
        this.memory.lastRecall = {
          at: env.at,
          items: env.items,
          tokens: env.tokens,
          omitted: env.omitted,
          budget: env.budget,
        }
        break
      case 'memory_maintenance_started':
        this.memory.maintenanceRunning = true
        break
      case 'memory_maintenance_finished':
        this.memory.maintenanceRunning = false
        this.memory.lastMaintenance = {
          at: env.at,
          outcome: env.outcome,
          report: env.report,
          error: env.error,
        }
        break
      case 'search_results':
        this.searchResult = {
          at: env.at,
          query: env.query,
          status: env.status,
          hits: env.hits,
          omitted: env.omitted,
          notes: env.notes ?? [],
          elapsedMs: env.elapsed_ms,
        }
        break
      case 'agent_profiles':
        this.agentProfiles = env
        break
      case 'skills':
        this.skillsListing = { at: env.at, listing: env.listing as SkillsListing }
        break
      case 'skill_activated':
        if (!env.agent_id) this.lastSkill = { at: env.at, name: env.name, ok: true }
        break
      case 'skill_activation_failed':
        if (!env.agent_id) this.lastSkill = { at: env.at, name: env.name, ok: false, reason: env.reason }
        this.notice('error', `Skill « ${env.name} » non activé : ${env.reason}`, runId)
        break
      case 'job_started':
        this.jobs.set(
          env.job_id,
          reactive<JobInfo>({
            jobId: env.job_id,
            agentId: env.agent_id,
            rootRunId: env.root_run_id,
            command: env.command,
            cwd: env.cwd,
            pid: env.pid,
            startedAt: env.at,
            stdoutBytes: 0,
            stderrBytes: 0,
            state: 'running',
            logs: [],
          }),
        )
        break
      case 'job_output': {
        const j = this.jobs.get(env.job_id)
        if (j) {
          j.stdoutBytes = env.stdout_bytes
          j.stderrBytes = env.stderr_bytes
        }
        break
      }
      case 'job_finished': {
        const j = this.jobs.get(env.job_id)
        if (j) {
          j.state = env.state
          j.code = env.code
          j.signal = env.signal
          j.durationMs = env.duration_ms
          j.logs = env.logs ?? []
        }
        break
      }
      case 'agent_control_result':
        this.lastAgentControl = { action: env.action, ok: env.ok, text: env.text, at: env.at }
        break
      case 'command_rejected':
        this.lastRejection = { command: env.command, reason: env.reason, at: env.at }
        break
      default:
        break
    }
  }

  private startRun(env: Extract<Envelope, { type: 'run_started' }>, live: boolean) {
    const runId = env.run_id ?? localId('run')
    this.runs.set(runId, reactive<RunInfo>({
      runId,
      prompt: env.prompt,
      model: env.model,
      reasoning: env.reasoning,
      startedAt: env.at,
      approvalsUnsatisfied: [],
      files: new SvelteMap(),
      commands: [],
    }))
    this.lastSeq = Math.max(this.lastSeq, env.seq)
    if (!live) return
    this.runningRunId = runId
    this.lastRejection = null
    this.items.push({
      kind: 'user',
      id: localId('u'),
      runId,
      text: env.prompt,
      attachments: env.attachments,
      at: env.at,
    })
  }

  private recordCommand(env: Extract<Envelope, { type: 'tool_finished' }>) {
    if (!SHELL_TOOLS.has(env.name)) return
    const run = this.run(env.run_id)
    if (!run) return
    const tool = this.findTool(env.tool_call_id, env.name)
    const input = (tool?.input ?? {}) as { command?: unknown }
    run.commands.push({
      toolCallId: env.tool_call_id,
      name: env.name,
      command: typeof input.command === 'string' ? input.command : '',
      isError: env.is_error,
      durationMs: env.duration_ms,
      output: env.output,
    })
  }

  private currentAssistant(runId: string | undefined): AssistantItem {
    const last = this.items.at(-1)
    if (last?.kind === 'assistant' && last.streaming && last.runId === runId) return last
    const item: AssistantItem = {
      kind: 'assistant',
      id: localId('a'),
      runId,
      text: '',
      thinking: '',
      streaming: true,
    }
    this.items.push(item)
    // Return the proxied element so later appends are reactive.
    return this.items.at(-1) as AssistantItem
  }

  private closeAssistant() {
    const last = this.items.at(-1)
    if (last?.kind === 'assistant') last.streaming = false
  }

  private findTool(toolCallId: string | undefined, name: string): ToolItem | undefined {
    return this.items.findLast(
      (i): i is ToolItem =>
        i.kind === 'tool' &&
        (toolCallId ? i.toolCallId === toolCallId : i.name === name && i.status === 'running'),
    )
  }

  private notice(level: NoticeItem['level'], message: string, runId?: string) {
    this.items.push({ kind: 'notice', id: localId('n'), runId, level, message })
  }
}
