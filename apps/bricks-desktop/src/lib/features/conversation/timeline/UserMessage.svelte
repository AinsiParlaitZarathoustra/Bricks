<script lang="ts">
  import { t } from '$lib/i18n/i18n.svelte'
  import Check from '@lucide/svelte/icons/check'
  import Copy from '@lucide/svelte/icons/copy'
  import Pencil from '@lucide/svelte/icons/pencil'
  import FileIcon from '$lib/features/file-icons/FileIcon.svelte'
  import { Button } from '$lib/components/ui/button'
  import { Textarea } from '$lib/components/ui/textarea'
  import * as Bubble from '$lib/components/ui/bubble'
  import * as Message from '$lib/components/ui/message'
  import * as Tooltip from '$lib/components/ui/tooltip'
  import MessageAction from './MessageAction.svelte'
  import { app } from '$lib/state/app.svelte'
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

  // « Copier »: the message's own text.
  let copied = $state(false)
  let copyError = $state<string | null>(null)
  let copyTimer: ReturnType<typeof setTimeout> | undefined
  $effect(() => () => clearTimeout(copyTimer))

  async function copy() {
    try {
      await navigator.clipboard.writeText(item.text)
      copyError = null
      copied = true
      clearTimeout(copyTimer)
      copyTimer = setTimeout(() => (copied = false), 2000)
    } catch (e) {
      copied = false
      copyError = e instanceof Error ? e.message : String(e)
    }
  }

  // « Modifier »: the text in a field, saved by replacing this turn and running again.
  let editing = $state(false)
  let draft = $state('')
  let saving = $state(false)
  let editError = $state<string | null>(null)
  let editButton = $state<HTMLElement | null>(null)

  const running = $derived(conversation?.running ?? false)
  const canSave = $derived(!saving && !running && (draft.trim() !== '' || item.attachments.length > 0))

  function focusField(element: HTMLTextAreaElement) {
    element.focus()
    element.setSelectionRange(element.value.length, element.value.length)
  }

  function startEdit() {
    draft = item.text
    editError = null
    editing = true
  }

  function cancelEdit() {
    editing = false
    editError = null
    editButton?.focus()
  }

  async function save() {
    if (!conversation || !canSave) return
    saving = true
    const error = await app.reviseMessage(conversation, item, draft)
    saving = false
    if (error) {
      editError = error
      return
    }
    editing = false
  }

  function onEditKeydown(event: KeyboardEvent) {
    if (event.isComposing) return
    if (event.key === 'Escape') {
      event.preventDefault()
      event.stopPropagation()
      cancelEdit()
    } else if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
      event.preventDefault()
      void save()
    }
  }

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
  {#if editing}
    <div class="flex w-full max-w-[85%] flex-col gap-2">
      <Textarea
        {@attach focusField}
        bind:value={draft}
        aria-label={t('conversation.messageEdited')}
        class="max-h-[40vh] min-h-20 text-sm"
        onkeydown={onEditKeydown}
      />
      {#if editError}
        <p class="text-xs break-words text-destructive" role="alert">{editError}</p>
      {/if}
      <div class="flex justify-end gap-2">
        <Button variant="outline" size="sm" onclick={cancelEdit}>{t('common.cancel')}</Button>
        <Button size="sm" disabled={!canSave} onclick={save}>{t('conversation.saveAndRerun')}</Button>
      </div>
    </div>
  {:else if item.text}
    <Message.Root align="end">
      <Message.Content class="gap-1">
        <Bubble.Root align="end" variant="muted" class="max-w-[85%]">
          <Bubble.Content dir="auto" class="text-start whitespace-pre-wrap"
            >{#if item.commandLabel}<span
                class="mr-1.5 inline-block rounded-sm bg-background/70 px-1.5 align-baseline font-mono text-xs text-muted-foreground"
                dir="ltr">{item.commandLabel}</span>{/if}{#each parts as part, i (i)}{#if part.kind === 'link'}<a
                  href={part.href}
                  rel="noopener noreferrer nofollow"
                  title={part.href}
                  class="rounded-sm text-primary underline underline-offset-2 hover:opacity-80 focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                  onclick={(event) => openLink(event, part.href)}
                  onauxclick={(event) => event.preventDefault()}>{part.text}</a
                >{:else}{part.text}{/if}{/each}</Bubble.Content
          >
        </Bubble.Root>
        <!-- Present at rest (no shift when they show), visible on hover, focus or touch. -->
        <Tooltip.Provider delayDuration={400}>
          <Message.Footer
            class="gap-0.5 px-0 opacity-0 transition-opacity group-hover/message:opacity-100 group-focus-within/message:opacity-100 [@media(hover:none)]:opacity-100"
          >
            <!-- A message sent by a typed command (`/skill …`) is sent again by typing it. -->
            {#if conversation && !conversation.readOnly && !item.commandLabel}
              <MessageAction bind:ref={editButton} label={t('conversation.editTheMessage')} disabled={running} onclick={startEdit}>
                <Pencil aria-hidden="true" />
              </MessageAction>
            {/if}
            <MessageAction label={t('conversation.copyTheMessage')} onclick={copy}>
              {#if copied}
                <Check aria-hidden="true" />
              {:else}
                <Copy aria-hidden="true" />
              {/if}
            </MessageAction>
          </Message.Footer>
        </Tooltip.Provider>
      </Message.Content>
    </Message.Root>
    {#if copyError}
      <p class="max-w-[85%] text-xs break-words text-destructive" role="alert">{t('conversation.couldNotCopy', { copyError })}</p>
    {/if}
    {#if linkError}
      <p class="max-w-[85%] text-xs break-words text-destructive" role="alert">
        {t('conversation.theLinkCouldNotBeOpened', { linkError })}
      </p>
    {/if}
  {/if}
  {#if item.attachments.length > 0}
    <ul class="flex max-w-[85%] flex-wrap justify-end gap-1.5" aria-label={t('common.attachments')}>
      {#each item.attachments as attachment, i (`${i}:${attachment.path}`)}
        {@const size = formatBytes(attachment.bytes)}
        {@const id = copyId(attachment)}
        {@const preview = id ? previews[id] : undefined}
        <li
          class="inline-flex max-w-full items-center gap-1.5 rounded-md border border-border bg-card px-2 py-1 text-xs text-card-foreground"
          title={shownName(attachment)}
        >
          {#if preview}
            <img src={preview} alt="" class="size-10 shrink-0 rounded-sm object-cover" />
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
