<script lang="ts">
  // A sub-agent's branch, in the centre: its work read like the main conversation
  // (same timeline), live while it runs, and nothing to write to it. The title bar
  // leads back to the conversation.
  import { t } from '$lib/i18n/i18n.svelte'
  import { untrack } from 'svelte'
  import ConversationTimeline from '$lib/features/conversation/ConversationTimeline.svelte'
  import type { AgentInfo, Conversation } from '$lib/state/conversation.svelte'
  import { agentBranch, applyAgentSteps } from './branch'

  let { conversation, agent }: { conversation: Conversation; agent: AgentInfo } = $props()

  // Built once (the view is keyed by the agent), then fed the steps as they
  // arrive: the timeline keeps its items, its scroll and what was unfolded.
  const branch = untrack(() => agentBranch(conversation, agent))
  let applied = untrack(() => agent.log.length)
  $effect(() => {
    const n = agent.log.length
    if (n <= applied) return
    untrack(() => applyAgentSteps(branch, agent, applied))
    applied = n
  })
</script>

<div class="flex min-h-0 flex-1 flex-col">
  <ConversationTimeline conversation={branch} />
</div>
<section aria-label={t('common.messageInput')} class="shrink-0 px-4 pt-1 pb-3">
  <p
    class="mx-auto mt-1 max-w-3xl rounded-2xl border bg-card px-4 py-3.5 text-ui text-muted-foreground shadow-[0_1px_2px_rgb(0_0_0/0.05),0_6px_20px_-6px_rgb(0_0_0/0.14)] dark:shadow-[0_1px_2px_rgb(0_0_0/0.4),0_8px_24px_-8px_rgb(0_0_0/0.6)]"
  >
    {t('agents.youCantWriteToASub')}
  </p>
</section>
