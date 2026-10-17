<script lang="ts">
  import type { Conversation } from '$lib/state/conversation.svelte'
  import ConversationTimeline from './ConversationTimeline.svelte'

  let { conversation }: { conversation: Conversation } = $props()
</script>

<div class="flex h-full min-h-0 flex-col">
  <ConversationTimeline {conversation} />
</div>
