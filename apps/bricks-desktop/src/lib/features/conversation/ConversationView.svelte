<script lang="ts">
  import type { Conversation } from '$lib/state/conversation.svelte'
  import ConversationTimeline from './ConversationTimeline.svelte'
  import PlanPanel from './PlanPanel.svelte'

  let { conversation }: { conversation: Conversation } = $props()
</script>

<div class="flex h-full min-h-0 flex-col">
  <ConversationTimeline {conversation} />
  {#if conversation.plan && conversation.plan.todos.length > 0}
    <div class="mx-auto w-full max-w-3xl shrink-0 px-4 pb-2">
      <PlanPanel plan={conversation.plan} />
    </div>
  {/if}
</div>
