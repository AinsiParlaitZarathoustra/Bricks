// A sub-agent's read-only branch: its work replayed as a conversation of its own
// (its mission, its steps and tool calls, its answer), so it reads exactly like
// the main conversation with the same components. Built from the steps the
// parent conversation keeps for it (`AgentInfo.log`); nothing is sent from it.
import type { Envelope, Snapshot } from '$lib/ipc/engine'
import { Conversation, type AgentInfo } from '$lib/state/conversation.svelte'
import { SCHEMA_VERSION } from '$lib/ipc/engine'

/** The run of one execution of a sub-agent, in its branch. */
export function branchRunId(agentId: string, execution: number): string {
  return `${agentId}:${execution}`
}

/** The name a sub-agent goes by: its label, else its profile. */
export function agentName(agent: Pick<AgentInfo, 'spawn'>): string {
  const label = agent.spawn.description?.trim()
  return label ? label : `Agent ${agent.spawn.profile}`
}

/** The branch of `agent`, as it stands now. */
export function agentBranch(parent: Conversation, agent: AgentInfo): Conversation {
  const zero = {
    input_tokens: 0,
    output_tokens: 0,
    total_tokens: 0,
    cache_creation_input_tokens: 0,
    cache_read_input_tokens: 0,
    reasoning_tokens: 0,
  }
  const snapshot = {
    session_id: parent.sessionId,
    title: agentName(agent),
    working_dir: agent.spawn.workspace,
    model: agent.spawn.model.applied,
    reasoning: null,
    effective_reasoning: null,
    activity: { state: 'idle' },
    maintenance_running: false,
    interactive: false,
    context: parent.context,
    usage: zero,
    pending_approvals: [],
    memory_space: null,
    permission_policy: parent.permissions?.policy,
    permission_revision: parent.permissions?.revision ?? 0,
  } as unknown as Snapshot
  const branch = new Conversation(
    {
      session: { ...parent.session, title: agentName(agent), running: false },
      snapshot,
      history: [],
      recent_events: [],
      last_seq: 0,
      run_labels: [],
      run_records: [],
    },
    parent.projectId,
  )
  branch.readOnly = true
  applyAgentSteps(branch, agent, 0)
  return branch
}

/** Replays the agent's steps from `from` (its log only grows) into its branch. */
export function applyAgentSteps(branch: Conversation, agent: AgentInfo, from: number): void {
  for (let i = from; i < agent.log.length; i++) {
    const step = agent.log[i]
    branch.apply({
      ...step.event,
      schema: SCHEMA_VERSION,
      session_id: branch.sessionId,
      run_id: branchRunId(agent.id, step.execution),
      seq: i + 1,
      at: step.at,
    } as Envelope)
  }
}
