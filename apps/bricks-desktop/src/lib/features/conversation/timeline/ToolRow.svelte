<script lang="ts">
  import type { Conversation, ToolItem, EditItem } from '$lib/state/conversation.svelte'
  import EditRow from './EditRow.svelte'
  import ToolLine from './ToolLine.svelte'
  import { toolLineState } from './toolLine'

  let { item, conversation }: { item: ToolItem; conversation: Conversation } = $props()

  /** The run that made the call was cancelled: its unfinished calls are interruptions. */
  const interrupted = $derived(conversation.run(item.runId)?.outcome === 'cancelled')
  const edits = $derived(conversation.items.filter((entry): entry is EditItem =>
    entry.kind === 'edit' && entry.toolCallId === item.toolCallId && entry.runId === item.runId && !entry.agentId
  ))
  const state = $derived(toolLineState(item, interrupted))
</script>

<ToolLine name={item.name} {state} durationMs={item.durationMs}>
  {#each edits as edit (edit.id)}
    <EditRow item={edit} {conversation} inline />
  {/each}
</ToolLine>
