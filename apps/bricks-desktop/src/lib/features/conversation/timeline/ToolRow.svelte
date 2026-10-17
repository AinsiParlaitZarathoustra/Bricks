<script lang="ts">
  import type { Conversation, ToolItem } from '$lib/state/conversation.svelte'
  import ToolLine from './ToolLine.svelte'
  import { toolLineState } from './toolLine'

  let { item, conversation }: { item: ToolItem; conversation: Conversation } = $props()

  /** The run that made the call was cancelled: its unfinished calls are interruptions. */
  const interrupted = $derived(conversation.run(item.runId)?.outcome === 'cancelled')
  const state = $derived(toolLineState(item, interrupted))
</script>

<ToolLine name={item.name} {state} durationMs={item.durationMs} />
