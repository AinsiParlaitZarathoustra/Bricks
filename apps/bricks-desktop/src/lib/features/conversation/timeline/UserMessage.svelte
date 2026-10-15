<script lang="ts">
  import { File, Folder, Image } from '@lucide/svelte'
  import type { UserItem } from '$lib/state/conversation.svelte'
  import { basename, formatBytes } from '../format'

  let { item }: { item: UserItem } = $props()
</script>

<div class="flex flex-col items-end gap-1.5">
  {#if item.text}
    <div class="max-w-[85%] rounded-2xl bg-secondary px-3.5 py-2 text-sm break-words whitespace-pre-wrap text-secondary-foreground">
      {item.text}
    </div>
  {/if}
  {#if item.attachments.length > 0}
    <ul class="flex max-w-[85%] flex-wrap justify-end gap-1.5" aria-label="Pièces jointes">
      {#each item.attachments as attachment, i (`${i}:${attachment.path}`)}
        {@const size = formatBytes(attachment.bytes)}
        <li
          class="inline-flex max-w-full items-center gap-1.5 rounded-md border border-border bg-card px-2 py-1 text-xs text-card-foreground"
          title={attachment.path}
        >
          {#if attachment.kind === 'image'}
            <Image class="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
          {:else if attachment.kind === 'folder'}
            <Folder class="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
          {:else}
            <File class="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
          {/if}
          <span class="min-w-0 truncate">{basename(attachment.path)}</span>
          {#if size}<span class="shrink-0 text-muted-foreground">{size}</span>{/if}
          {#if attachment.note}<span class="shrink-0 text-muted-foreground">· {attachment.note}</span>{/if}
        </li>
      {/each}
    </ul>
  {/if}
</div>
