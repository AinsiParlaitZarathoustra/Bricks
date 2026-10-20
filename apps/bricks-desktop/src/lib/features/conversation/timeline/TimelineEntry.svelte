<script lang="ts">
  import type { Conversation, TimelineItem } from '$lib/state/conversation.svelte'
  import UserMessage from './UserMessage.svelte'
  import AssistantMessage from './AssistantMessage.svelte'
  import ToolRow from './ToolRow.svelte'
  import ApprovalCard from './ApprovalCard.svelte'
  import EditRow from './EditRow.svelte'
  import NoticeLine from './NoticeLine.svelte'
  import RunEndCard from './RunEndCard.svelte'
  import ResponseCopy from './ResponseCopy.svelte'

  let { item, conversation }: { item: TimelineItem; conversation: Conversation } = $props()

  /** The response this item ends, when it is complete and has text: its copy action follows the item. */
  const response = $derived(conversation.responseByAnchor(item.id))
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
  {#if item.agentId || !conversation.items.some(entry => entry.kind === 'tool' && entry.toolCallId === item.toolCallId && entry.runId === item.runId)}
    <EditRow {item} {conversation} />
  {/if}
{:else if item.kind === 'notice'}
  <NoticeLine {item} />
{:else if item.kind === 'run_end'}
  <RunEndCard {item} {conversation} />
{/if}

{#if response && response.complete && response.copyText.trim()}
  <ResponseCopy text={response.copyText} />
{/if}
