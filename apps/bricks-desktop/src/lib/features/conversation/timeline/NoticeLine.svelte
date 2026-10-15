<script lang="ts">
  import { CircleAlert, Info, TriangleAlert } from '@lucide/svelte'
  import type { NoticeItem } from '$lib/state/conversation.svelte'

  let { item }: { item: NoticeItem } = $props()
</script>

<p
  class="flex items-start gap-2 text-xs break-words whitespace-pre-wrap {item.level === 'error'
    ? 'text-destructive'
    : item.level === 'warning'
      ? 'text-warning'
      : 'text-muted-foreground'}"
>
  {#if item.level === 'error'}
    <CircleAlert class="mt-px size-3.5 shrink-0" aria-hidden="true" />
  {:else if item.level === 'warning'}
    <TriangleAlert class="mt-px size-3.5 shrink-0" aria-hidden="true" />
  {:else}
    <Info class="mt-px size-3.5 shrink-0" aria-hidden="true" />
  {/if}
  <span class="min-w-0">{item.message}</span>
</p>
