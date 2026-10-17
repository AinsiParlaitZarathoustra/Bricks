<script lang="ts">
  import File from '@lucide/svelte/icons/file'
  import Folder from '@lucide/svelte/icons/folder'
  import Image from '@lucide/svelte/icons/image'
  import { openUrl } from '@tauri-apps/plugin-opener'
  import type { UserItem } from '$lib/state/conversation.svelte'
  import { basename, formatBytes } from '../format'
  import { isOpenableUrl, messageParts } from '../links'

  let { item }: { item: UserItem } = $props()

  // The text is plain: its web addresses are link parts, rendered as markup.
  const parts = $derived(messageParts(item.text))
  /** Error of the last link that could not be opened; cleared by the next successful open. */
  let linkError = $state<string | null>(null)

  function openLink(event: MouseEvent, href: string) {
    // Never let the webview navigate: the address goes to the system handler.
    event.preventDefault()
    if (!isOpenableUrl(href)) return
    openUrl(href).then(
      () => (linkError = null),
      (error: unknown) => (linkError = error instanceof Error ? error.message : String(error)),
    )
  }
</script>

<div class="flex flex-col items-end gap-1.5">
  {#if item.text}
    <div
      dir="auto"
      class="max-w-[85%] rounded-2xl bg-secondary px-3.5 py-2 text-start text-sm break-words whitespace-pre-wrap text-secondary-foreground"
    >{#each parts as part, i (i)}{#if part.kind === 'link'}<a
            href={part.href}
            rel="noopener noreferrer nofollow"
            title={part.href}
            class="rounded-sm text-primary underline underline-offset-2 hover:opacity-80 focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
            onclick={(event) => openLink(event, part.href)}
            onauxclick={(event) => event.preventDefault()}>{part.text}</a>{:else}{part.text}{/if}{/each}</div>
    {#if linkError}
      <p class="max-w-[85%] text-xs break-words text-destructive" role="alert">
        Le lien n’a pas pu être ouvert : {linkError}
      </p>
    {/if}
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
