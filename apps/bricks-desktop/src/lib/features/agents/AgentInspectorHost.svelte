<script lang="ts">
  // The inspector of the sub-agent chosen in the thread, for the shown conversation.
  import { untrack } from 'svelte'
  import { app } from '$lib/state/app.svelte'
  import { ui } from '$lib/state/ui.svelte'
  import AgentInspector from './AgentInspector.svelte'
  import { isLive } from './agentView'

  const conversation = $derived(app.current)
  // The inspector belongs to the conversation it was opened in.
  let openedIn: string | null = null
  $effect(() => {
    const sessionId = conversation?.sessionId ?? null
    if (openedIn !== null && sessionId !== openedIn) ui.closeAgent()
    openedIn = ui.agentId ? sessionId : null
  })
  const agent = $derived(ui.agentId && conversation ? (conversation.agents.get(ui.agentId) ?? null) : null)

  let now = $state(Date.now())
  $effect(() => {
    if (!agent || !isLive(agent.state)) return
    untrack(() => (now = Date.now()))
    const id = setInterval(() => (now = Date.now()), 1000)
    return () => clearInterval(id)
  })
</script>

{#if agent && conversation}
  <AgentInspector {agent} {conversation} {now} onClose={() => ui.closeAgent()} />
{/if}
