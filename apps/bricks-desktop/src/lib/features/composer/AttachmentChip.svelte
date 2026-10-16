<script lang="ts">
  import type { DraftAttachment } from '$lib/state/conversation.svelte'
  import { Button } from '$lib/components/ui/button'
  import FileIcon from '@lucide/svelte/icons/file'
  import FolderIcon from '@lucide/svelte/icons/folder'
  import ImageIcon from '@lucide/svelte/icons/image'
  import XIcon from '@lucide/svelte/icons/x'

  let {
    attachment,
    error,
    onRemove,
  }: {
    attachment: DraftAttachment
    /** Error to show on the chip: a backend refusal or a modality refusal. */
    error: string | null
    onRemove: () => void
  } = $props()
</script>

<div
  class="flex max-w-full items-center gap-2 rounded-md border bg-muted/60 py-1 pr-1 pl-1.5 text-sm"
  class:border-transparent={error === null}
  class:border-destructive={error !== null}
  data-kind={attachment.kind}
>
  {#if attachment.previewUrl}
    <img src={attachment.previewUrl} alt="" class="size-8 shrink-0 rounded object-cover" />
  {:else if attachment.kind === 'folder'}
    <FolderIcon class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
  {:else if attachment.kind === 'image'}
    <ImageIcon class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
  {:else}
    <FileIcon class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
  {/if}

  <div class="min-w-0">
    <p class="max-w-56 truncate" title={attachment.path || attachment.name}>{attachment.name}</p>
    {#if error}
      <p class="text-xs text-destructive" role="alert">{error}</p>
    {/if}
  </div>

  <Button variant="ghost" size="icon-xs" aria-label={`Retirer ${attachment.name}`} onclick={onRemove}>
    <XIcon aria-hidden="true" />
  </Button>
</div>
