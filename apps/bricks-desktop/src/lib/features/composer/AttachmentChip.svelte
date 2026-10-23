<script lang="ts">
  import type { DraftAttachment } from '$lib/state/conversation.svelte'
  import { Button } from '$lib/components/ui/button'
  import FileIcon from '$lib/features/file-icons/FileIcon.svelte'
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
  aria-busy={attachment.importing ? 'true' : undefined}
  class="flex max-w-full items-center gap-2 rounded-md border bg-muted/60 py-1 pr-1 pl-1.5 text-sm"
  class:border-transparent={error === null}
  class:border-destructive={error !== null}
  data-kind={attachment.kind}
>
  <div class="flex min-w-0 items-center {attachment.previewUrl ? 'gap-2' : 'gap-1.5'}">
    {#if attachment.previewUrl}
      <img src={attachment.previewUrl} alt="" class="size-8 shrink-0 rounded object-cover" />
    {:else}
      <FileIcon path={attachment.path || attachment.name} kind={attachment.kind === 'folder' ? 'folder' : 'file'} />
    {/if}

    <div class="min-w-0">
      <p class="max-w-56 truncate" title={attachment.path || attachment.name}>{attachment.name}</p>
      {#if error}
        <p class="text-xs text-destructive" role="alert">{error}</p>
      {:else if attachment.importing}
        <p class="text-xs text-muted-foreground">Copie…</p>
      {/if}
    </div>
  </div>

  <Button variant="ghost" size="icon-xs" aria-label={`Retirer ${attachment.name}`} onclick={onRemove}>
    <XIcon aria-hidden="true" />
  </Button>
</div>
