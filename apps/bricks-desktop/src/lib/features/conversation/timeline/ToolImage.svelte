<script lang="ts">
  // The image a tool call showed the model (`ViewImage`): the bytes it received,
  // read from the session's history. The round's message is written once all its
  // calls end, so the image is asked for again as the timeline grows, until found.
  import { sessionToolImage } from '$lib/ipc/client'
  import type { Conversation, ToolItem } from '$lib/state/conversation.svelte'
  import { basename, isRecord } from '../format'

  let { item, conversation }: { item: ToolItem; conversation: Conversation } = $props()

  let src = $state<string | null>(null)

  const path = $derived(
    isRecord(item.input) && typeof item.input.file_path === 'string' ? item.input.file_path : '',
  )

  $effect(() => {
    if (src) return
    void conversation.items.length
    const sessionId = conversation.sessionId
    const callId = item.toolCallId
    let current = true
    sessionToolImage(sessionId, callId)
      .then((url) => {
        if (current && url) src = url
      })
      .catch(() => {})
    return () => {
      current = false
    }
  })
</script>

{#if src}
  <img
    {src}
    alt={path ? basename(path) : 'Image vue par le modèle'}
    title={path}
    class="mt-1 ml-5 max-h-64 max-w-[min(100%,28rem)] rounded-md border border-border object-contain"
  />
{/if}
