<script lang="ts">
  import type { Conversation, TimelineItem } from '$lib/state/conversation.svelte'
  import UserMessage from './UserMessage.svelte'
  import AssistantMessage from './AssistantMessage.svelte'
  import ToolRow from './ToolRow.svelte'
  import ApprovalCard from './ApprovalCard.svelte'
  import EditRow from './EditRow.svelte'
  import NoticeLine from './NoticeLine.svelte'
  import RunEndCard from './RunEndCard.svelte'

  let { item, conversation }: { item: TimelineItem; conversation: Conversation } = $props()
</script>

{#if item.kind === 'user'}
  <UserMessage {item} />
{:else if item.kind === 'assistant'}
  <AssistantMessage {item} {conversation} />
{:else if item.kind === 'tool'}
  <ToolRow {item} {conversation} />
{:else if item.kind === 'approval'}
  <ApprovalCard approval={item.approval} resolved={item.resolved} {conversation} />
{:else if item.kind === 'edit'}
  <EditRow {item} {conversation} />
{:else if item.kind === 'notice'}
  <NoticeLine {item} />
{:else if item.kind === 'run_end'}
  <RunEndCard {item} {conversation} />
{/if}
