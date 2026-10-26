<script lang="ts">
  // One sub-agent in the thread: its state, profile, short description and the
  // time it has worked; what it is doing now on a second, discreet line. The line
  // opens its inspector. No model, reasoning, counts or cost here.
  import { untrack } from 'svelte'
  import StatusDot from '$lib/layout/StatusDot.svelte'
  import type { AgentItem, Conversation } from '$lib/state/conversation.svelte'
  import { ui } from '$lib/state/ui.svelte'
  import { agentAction, agentDurationMs, agentStateView, formatDuration, isLive } from '$lib/features/agents/agentView'

  let { item, conversation }: { item: AgentItem; conversation: Conversation } = $props()

  const agent = $derived(conversation.agents.get(item.agentId) ?? null)
  const live = $derived(agent ? isLive(agent.state) : false)

  let now = $state(Date.now())
  $effect(() => {
    if (!live) return
    untrack(() => (now = Date.now()))
    const id = setInterval(() => (now = Date.now()), 1000)
    return () => clearInterval(id)
  })

  const awaitingApproval = $derived(
    conversation.items.some(
      (i) => i.kind === 'approval' && !i.resolved && i.approval.agent_id === item.agentId,
    ),
  )
  const view = $derived(agent ? agentStateView(agent.state) : null)
  const action = $derived(agent ? agentAction(agent, awaitingApproval) : null)
  const description = $derived(agent ? agent.spawn.description || agent.spawn.task : '')
  /** A resumed agent shows its current execution once, where it was resumed. */
  const stale = $derived(agent !== null && item.execution !== agent.execution)
</script>

{#if agent && view}
  <button
    type="button"
    class="group flex w-full min-w-0 items-start gap-2.5 rounded-md px-2 py-1.5 text-start outline-none hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring"
    aria-label={`Sous-agent ${agent.spawn.profile} : ${description}`}
    onclick={() => ui.openAgent(agent.id)}
  >
    <StatusDot
      status={stale ? 'idle' : awaitingApproval ? 'waiting' : view.dot}
      label={stale ? 'Exécution précédente (reprise plus loin)' : awaitingApproval ? 'En attente d’autorisation' : view.label}
      class="mt-[0.4rem] shrink-0"
    />
    <span class="flex min-w-0 flex-1 flex-col gap-0.5">
      <span class="flex min-w-0 items-baseline gap-2.5">
        <span class="shrink-0 font-mono text-ui text-foreground">{agent.spawn.profile}</span>
        <span class="min-w-0 flex-1 truncate text-ui text-muted-foreground">{description}</span>
        {#if !stale}
          <span class="shrink-0 font-mono text-xs text-muted-foreground tabular-nums">
            {formatDuration(agentDurationMs(agent, now))}
          </span>
        {/if}
      </span>
      {#if action && !stale}
        <span class="truncate font-mono text-xs text-muted-foreground" aria-live="off">{action}</span>
      {/if}
    </span>
  </button>
{/if}
