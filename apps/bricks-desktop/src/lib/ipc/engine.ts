// Engine protocol types, mirroring `cersei_agent::control::protocol`
// (schema 5, see docs/cli.md). These are the JSON shapes the engine
// serializes; the desktop bridge forwards them unchanged.
// Owner: orchestrator. Keep in sync with protocol.rs.

export const SCHEMA_VERSION = 5

export type RunOutcome = 'succeeded' | 'incomplete' | 'failed' | 'cancelled'
export type FailureKind = 'approval_required' | 'error'
export type Decision = 'allow' | 'allow_for_session' | 'deny'
export type DecidedBy = 'user' | 'session' | 'non_interactive' | 'cancelled'
export type ChangeKind = 'create' | 'modify' | 'delete'
export type Provenance = 'measured' | 'counted' | 'mixed' | 'estimated'
export type InstanceState =
  | 'created'
  | 'queued'
  | 'waiting_admission'
  | 'starting'
  | 'running'
  | 'cancelling'
  | 'completed'
  | 'incomplete'
  | 'failed'
  | 'cancelled'
  | 'interrupted'
export type ChangeSetState = 'ready' | 'applied' | 'conflict' | 'discarded'

export const TERMINAL_AGENT_STATES: readonly InstanceState[] = [
  'completed',
  'incomplete',
  'failed',
  'cancelled',
  'interrupted',
]

export type Termination =
  | { kind: 'completed' }
  | { kind: 'max_turns'; limit: number }
  | { kind: 'output_truncated'; continuations: number }
  | { kind: 'no_progress'; repeats: number }
  | { kind: string; [k: string]: unknown }

export interface CostEstimate {
  amount_usd: number
  tariff: string
  /** Some responses had no price: `amount_usd` is a lower bound. */
  partial: boolean
  unpriced: string[]
}

export interface Usage {
  input_tokens: number
  output_tokens: number
  total_tokens: number
  cache_creation_input_tokens: number
  cache_read_input_tokens: number
  reasoning_tokens: number
  /** Absent: no price configured (unknown, not zero). */
  cost_usd?: number
  cost_estimate?: CostEstimate
}

export interface ContextUsed {
  tokens: number
  upper_bound: number
  measured_tokens: number
  estimated_tokens: number
  provenance: Provenance
  method: string
}

export interface ContextWindow {
  model: string
  /** Total window when known. */
  total: number | null
  max_input_tokens: number
  max_output_tokens: number | null
}

export interface SessionTotals {
  input_tokens: number
  cache_read_tokens: number
  cache_write_tokens: number
  output_tokens: number
  reasoning_tokens: number
  requests: number
  requests_without_usage: number
  compaction_requests: number
}

export interface ContextStatus {
  /** Active occupation: use this for the context ring. */
  context_used: ContextUsed
  context_window: ContextWindow
  input_limit: number
  reserved_output: number
  margin: number
  /** Cumulative consumption: NOT the occupation. */
  total_tokens: number
  totals: SessionTotals
  notes: string[]
}

export interface FileChange {
  path: string
  absolute: string
  kind: ChangeKind
  before_sha256: string | null
  diff: string
  added: number
  removed: number
}

export interface ChangePreview {
  files: FileChange[]
  refusal: string | null
}

export interface ApprovalRequest {
  approval_id: string
  tool_call_id: string
  tool: string
  level: string
  description: string
  input: unknown
  preview: ChangePreview | null
  agent_id?: string
}

export interface AttachmentInfo {
  kind: string
  path: string
  bytes: number
  sha256?: string
  note?: string
}

export interface WrittenFile {
  path: string
  kind: ChangeKind
  added: number
  removed: number
  binary?: boolean
}

export interface Choice {
  requested: string
  applied: string
  reason?: string
}

export interface SpawnInfo {
  agent_id: string
  parent_id?: string
  root_run_id: string
  tool_call_id?: string
  profile: string
  profile_source: string
  profile_revision: string
  model: Choice
  reasoning: Choice
  workspace: string
  isolation: string
  task: string
  description?: string
  created_at: string
  batch_index?: number
  background: boolean
  depth: number
  branch?: string
  skills?: { name: string; state: string; source?: string; [k: string]: unknown }[]
}

/** A sub-agent as the agent runtime keeps it (durable across runs). */
export interface InstanceRecord {
  info: SpawnInfo
  state: InstanceState
  reason?: string
  result?: AgentResult
  background: boolean
  updated_at_ms: number
}

export interface CommandRun {
  tool: string
  command: string
  is_error: boolean
  duration_ms: number
}

export interface AgentResult {
  agent_id: string
  profile: string
  status: string
  termination?: Termination
  error?: string
  summary: string
  summary_truncated: boolean
  files_changed: string[]
  commands: CommandRun[]
  warnings: string[]
  turns: number
  tool_calls: number
  usage: Usage
  duration_ms: number
  model: string
  [k: string]: unknown
}

export interface ChangedFile {
  path: string
  status: string
  from?: string
  added: number | null
  removed: number | null
  binary: boolean
}

export interface ChangeSet {
  id: string
  agent_id: string
  task: string
  workspace: string
  branch: string
  base_sha: string
  snapshot_id: string
  baseline_tree: string
  final_tree: string
  files: ChangedFile[]
  patch: string
  patch_bytes: number
  state: ChangeSetState
  validations: CommandRun[]
  note?: string
}

export interface SkillListing {
  name: string
  source: string
  state: string
  description?: string
  path?: string
  revision?: string
  details?: string[]
}

export interface SkillsListing {
  generation: number
  project_decision: string
  entries: SkillListing[]
  total: number
  page: number
  diagnostics?: string[]
  catalog_omitted: number
}

export interface ProfileListing {
  name: string
  description: string
  /** `project` | `user` | `built_in` (engine `ProfileScope`). */
  scope: string
  source: string
  valid: boolean
  error?: string
  shadows?: string[]
}

export type AgentProfilesEvent = Extract<Event, { type: 'agent_profiles' }>

export interface SkillSourceInfo {
  source: string
  path: string
  skills: number
}

export interface SearchHit {
  path: string
  line: number
  column: number
  text: string
}

export type Event =
  | {
      type: 'session_opened'
      working_dir: string
      model: string
      reasoning?: string
      effective_reasoning?: string
      resumed: boolean
      message_count: number
      warnings: string[]
    }
  | {
      type: 'run_started'
      prompt: string
      attachments: AttachmentInfo[]
      model: string
      reasoning?: string
    }
  | { type: 'text_delta'; text: string }
  | { type: 'thinking_delta'; text: string }
  | { type: 'tool_started'; tool_call_id: string; name: string; input: unknown }
  | { type: 'tool_progress'; tool_call_id?: string; name: string; message: string }
  | {
      type: 'tool_finished'
      tool_call_id: string
      name: string
      is_error: boolean
      duration_ms: number
      output: string
    }
  | { type: 'approval_requested'; approval: ApprovalRequest }
  | {
      type: 'approval_resolved'
      approval_id: string
      tool_call_id: string
      decision: Decision
      by: DecidedBy
    }
  | {
      type: 'edit_applied'
      tool_call_id: string
      tool: string
      files: WrittenFile[]
      agent_id?: string
      changeset_id?: string
    }
  | { type: 'memory_recalled'; items: number; tokens: number; omitted: number; budget: number }
  | { type: 'context'; status: ContextStatus }
  | { type: 'usage'; turn: Usage; total: Usage }
  | { type: 'compaction'; reason: string; outcome: string; compacted: boolean }
  | {
      type: 'model_changed'
      model: string
      reasoning?: string
      effective_reasoning?: string
      /** `next_turn` during a run, `next_run` otherwise. */
      applies: string
    }
  | {
      type: 'reasoning_changed'
      model: string
      requested?: string
      previous?: string
      previous_label?: string
      effective?: string
      effective_label?: string
      applies: string
    }
  | { type: 'context_cleared'; messages_removed: number }
  | { type: 'session_saved' }
  | { type: 'notice'; message: string }
  | {
      type: 'run_finished'
      outcome: RunOutcome
      failure?: FailureKind
      error?: string
      termination?: Termination
      text: string
      turns: number
      approvals_unsatisfied?: ApprovalRequest[]
    }
  | { type: 'memory_maintenance_started' }
  | {
      type: 'memory_maintenance_finished'
      outcome: 'completed' | 'cancelled' | 'failed'
      report?: unknown
      error?: string
    }
  | {
      type: 'search_results'
      query: string
      status: string
      hits: SearchHit[]
      omitted: number
      notes?: string[]
      elapsed_ms: number
    }
  | { type: 'agent_spawned'; agent: SpawnInfo }
  | { type: 'agent_state'; agent_id: string; state: InstanceState; reason?: string }
  | {
      type: 'agent_tool_started'
      agent_id: string
      tool_call_id: string
      name: string
      input: unknown
    }
  | {
      type: 'agent_tool_finished'
      agent_id: string
      tool_call_id: string
      name: string
      is_error: boolean
      duration_ms: number
    }
  | { type: 'agent_finished'; result: AgentResult }
  | { type: 'changes_ready'; changeset: ChangeSet }
  | {
      type: 'changes_updated'
      changeset_id: string
      state: ChangeSetState
      files?: string[]
      detail?: string
    }
  | {
      type: 'run_usage'
      root_run_id: string
      own: Usage
      descendants: Usage
      total: Usage
      pending_agents: number
      final_total: boolean
    }
  | {
      type: 'job_started'
      job_id: string
      agent_id: string
      root_run_id: string
      command: string
      cwd: string
      pid: number
    }
  | { type: 'job_output'; job_id: string; stdout_bytes: number; stderr_bytes: number }
  | {
      type: 'job_finished'
      job_id: string
      state: string
      code?: number
      signal?: number
      duration_ms: number
      logs?: string[]
    }
  | { type: 'agent_control_result'; action: string; ok: boolean; text: string }
  | {
      type: 'agent_profiles'
      profiles: ProfileListing[]
      total: number
      page: number
      diagnostics?: { source: string; message: string }[]
    }
  | {
      type: 'skill_activated'
      agent_id?: string
      name: string
      source: string
      revision: string
      origin: string
    }
  | { type: 'skill_activation_failed'; agent_id?: string; name: string; reason: string }
  | { type: 'skills'; listing: SkillsListing }
  | { type: 'skills_trust_requested'; project: string; sources: SkillSourceInfo[] }
  | { type: 'skills_trust_resolved'; project: string; allowed: boolean; by: string }
  | { type: 'command_rejected'; command: string; reason: string }

export type EventType = Event['type']

/** One delivered event: identifiers are flattened next to the event. */
export type Envelope = Event & {
  schema: number
  session_id: string
  run_id?: string
  seq: number
  /** Milliseconds since the Unix epoch. */
  at: number
}

export type PromptBlock =
  | { kind: 'text'; text: string }
  | { kind: 'file'; path: string }
  | { kind: 'folder'; path: string }
  | { kind: 'image'; path: string }

export interface Prompt {
  blocks: PromptBlock[]
  skills?: string[]
}

export type Command =
  | { type: 'submit'; prompt: Prompt }
  | { type: 'cancel' }
  | { type: 'set_model'; model?: string; reasoning?: string }
  | { type: 'set_reasoning'; profile: string | null }
  | { type: 'compact' }
  | { type: 'clear_context' }
  | { type: 'resume'; session_id: string }
  | { type: 'search'; text: string; regex?: boolean }
  | { type: 'list_agent_profiles'; query?: string; page?: number }
  | { type: 'reload_agent_profiles' }
  | {
      type: 'agent_control'
      action: string
      agent_id?: string
      changeset_id?: string
      job_id?: string
    }
  | { type: 'list_skills'; query?: string; page?: number }
  | { type: 'reload_skills' }
  | { type: 'trust_project_skills'; allow: boolean }
  | { type: 'approve'; approval_id: string; decision: Decision; reason?: string }

export type Activity =
  | { state: 'idle' }
  | { state: 'running'; run_id: string }
  | { state: 'busy'; what: string }

export interface Snapshot {
  session_id: string
  title: string
  working_dir: string
  model: string
  reasoning: string | null
  effective_reasoning: string | null
  activity: Activity
  maintenance_running: boolean
  interactive: boolean
  context: ContextStatus
  usage: Usage
  pending_approvals: ApprovalRequest[]
  allowed_for_session: string[]
  memory_space: string | null
  approval_rules: unknown
  /** Application-wide approval override (`standard` without one). */
  approval_mode: 'standard' | 'always_ask' | 'no_confirmation'
}

export interface ProfileChoice {
  id: string
  label: string
}

export interface ModelChoice {
  /** `provider_id/model_id`. */
  selection: string
  name: string
  provider: string
  profiles: ProfileChoice[]
  default_profile: string | null
  max_input_tokens: number
  context_window_tokens: number | null
  /** Input modalities (`text`, `image`, ...). */
  input: string[]
  priced: boolean
}
