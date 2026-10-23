<script lang="ts">
  import FileIcon from '$lib/features/file-icons/FileIcon.svelte'
  import { openUrl } from '@tauri-apps/plugin-opener'
  import type { Conversation, UserItem } from '$lib/state/conversation.svelte'
  import type { AttachmentInfo } from '$lib/ipc/engine'
  import { attachmentPreview } from '$lib/ipc/client'
  import { basename, formatBytes } from '../format'
  import { isOpenableUrl, messageParts } from '../links'

  let { item, conversation = null }: { item: UserItem; conversation?: Conversation | null } = $props()

  /** A copy kept by the desktop: `<id>-<name>` in the conversation's folder. */
  const COPY = /^([0-9a-f]{32})-(.+)$/
  function copyId(a: AttachmentInfo): string | null {
    return a.attachment_id ?? COPY.exec(basename(a.path))?.[1] ?? null
  }
  function shownName(a: AttachmentInfo): string {
    const name = basename(a.path)
    return COPY.exec(name)?.[2] ?? name
  }

  // Previews of the images, read from the conversation's copies.
  let previews = $state<Record<string, string>>({})
  $effect(() => {
    const sessionId = conversation?.sessionId
    if (!sessionId) return
    for (const a of item.attachments) {
      const id = a.kind === 'image' ? copyId(a) : null
      if (!id || previews[id]) continue
      attachmentPreview('session', sessionId, id)
        .then((p) => {
          if (p.data_url) previews = { ...previews, [id]: p.data_url }
        })
        .catch(() => {})
    }
  })

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
    >{#if item.commandLabel}<span
          class="mr-1.5 inline-block rounded bg-background/70 px-1.5 align-baseline font-mono text-xs text-muted-foreground"
          dir="ltr">{item.commandLabel}</span>{/if}{#each parts as part, i (i)}{#if part.kind === 'link'}<a
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
        {@const id = copyId(attachment)}
        {@const preview = id ? previews[id] : undefined}
        <li
          class="inline-flex max-w-full items-center gap-1.5 rounded-md border border-border bg-card px-2 py-1 text-xs text-card-foreground"
          title={shownName(attachment)}
        >
          {#if preview}
            <img src={preview} alt="" class="size-10 shrink-0 rounded object-cover" />
          {:else}
            <FileIcon path={shownName(attachment)} kind={attachment.kind === 'folder' ? 'folder' : 'file'} />
          {/if}
          <span class="min-w-0 truncate">{shownName(attachment)}</span>
          {#if size}<span class="shrink-0 text-muted-foreground">{size}</span>{/if}
          {#if attachment.note}<span class="shrink-0 text-muted-foreground">· {attachment.note}</span>{/if}
        </li>
      {/each}
    </ul>
  {/if}
</div>
