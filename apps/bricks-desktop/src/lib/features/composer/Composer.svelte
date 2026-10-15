<script lang="ts">
  import { onMount, untrack } from 'svelte'
  import { getCurrentWebview } from '@tauri-apps/api/webview'
  import type { DragDropEvent } from '@tauri-apps/api/webview'
  import { Button } from '$lib/components/ui/button'
  import { Textarea } from '$lib/components/ui/textarea'
  import { app } from '$lib/state/app.svelte'
  import type { Conversation, Draft, DraftAttachment } from '$lib/state/conversation.svelte'
  import { attachmentSave, errorMessage, pathKind } from '$lib/ipc/client'
  import type { PromptBlock } from '$lib/ipc/engine'
  import ArrowUpIcon from '@lucide/svelte/icons/arrow-up'
  import SquareIcon from '@lucide/svelte/icons/square'
  import XIcon from '@lucide/svelte/icons/x'
  import ProjectStrip from './ProjectStrip.svelte'
  import AttachMenu from './AttachMenu.svelte'
  import AttachmentChip from './AttachmentChip.svelte'
  import ModelSelect from './ModelSelect.svelte'
  import EffortSelect from './EffortSelect.svelte'
  import { basename, blobToBase64, extensionFor, isImagePath } from './attachments'

  let { conversation }: { conversation: Conversation | null } = $props()

  const NO_IMAGE_MESSAGE = 'Ce modèle n’accepte pas les images : retirez cette pièce jointe.'
  // Browsers without CSS field-sizing get the manual height fallback below.
  const HAS_FIELD_SIZING = typeof CSS !== 'undefined' && CSS.supports('field-sizing', 'content')

  // ─── Draft ────────────────────────────────────────────────────────────────
  // An open conversation keeps its own draft (it survives switches). Without
  // one, a local draft is used until the first send creates the conversation.
  let localDraft = $state<Draft>({ text: '', attachments: [] })
  const draft: Draft = $derived(conversation ? conversation.draft : localDraft)

  function revoke(a: DraftAttachment) {
    if (a.previewUrl) URL.revokeObjectURL(a.previewUrl)
  }

  function clearDraft(d: Draft) {
    for (const a of d.attachments) revoke(a)
    d.text = ''
    d.attachments = []
  }

  // ─── Model and effort ─────────────────────────────────────────────────────
  // Without a conversation, these choices are used by the first send.
  let pickedModel = $state<string | null>(null)
  let pickedEffort = $state<string | null>(null)

  const hasModels = $derived(app.models.length > 0)
  const selection: string | null = $derived(
    conversation ? conversation.model : (pickedModel ?? app.models[0]?.selection ?? null),
  )
  const choice = $derived(app.model(selection))
  const effortValue: string | null = $derived(conversation ? conversation.reasoning : pickedEffort)
  const effortEffective: string | null = $derived(
    conversation ? conversation.effectiveReasoning : (pickedEffort ?? choice?.default_profile ?? null),
  )
  const supportsImage = $derived(choice?.input.includes('image') ?? false)

  // ─── State flags ──────────────────────────────────────────────────────────
  const project = $derived(app.currentProject)
  const editable = $derived(project !== null && !project.missing && hasModels)
  const running = $derived(conversation?.running ?? false)

  let saving = $state(0)
  let sending = $state(false)
  let dragging = $state(false)
  let refusal = $state<string | null>(null)
  let rootEl = $state<HTMLElement | null>(null)
  let editorEl = $state<HTMLElement | null>(null)

  /** Why an attachment cannot be sent, or `null` when it can. */
  function chipError(a: DraftAttachment): string | null {
    if (a.error) return a.error
    if (a.kind === 'image' && !supportsImage) return NO_IMAGE_MESSAGE
    return null
  }

  const blocked = $derived(draft.attachments.some((a) => chipError(a) !== null))
  const hasContent = $derived(draft.text.trim() !== '' || draft.attachments.length > 0)
  const canSend = $derived(editable && !running && !sending && saving === 0 && !blocked && hasContent)

  const pendingText = $derived.by(() => {
    const p = conversation?.pendingChange
    if (!p) return null
    const what = p.what === 'model' ? 'Changement de modèle' : 'Changement d’effort'
    const when = p.applies === 'next_turn' ? 'au prochain tour' : 'au prochain run'
    return `${what} : s’applique ${when}.`
  })

  const placeholder = $derived.by(() => {
    if (!project) return 'Créez un projet pour écrire.'
    if (project.missing) return 'Le dossier du projet est introuvable.'
    if (!hasModels) return 'Aucun modèle configuré.'
    if (running) return 'Préparez le message suivant…'
    return 'Écrivez un message… (Entrée pour envoyer, Maj+Entrée pour une nouvelle ligne)'
  })

  // ─── Attachments ──────────────────────────────────────────────────────────

  function toBlock(a: DraftAttachment): PromptBlock {
    switch (a.kind) {
      case 'image':
        return { kind: 'image', path: a.path }
      case 'folder':
        return { kind: 'folder', path: a.path }
      default:
        return { kind: 'file', path: a.path }
    }
  }

  /** Items picked in the native dialog: the kind is known from the menu entry. */
  function addPicked(paths: string[], folder: boolean) {
    if (!editable || sending) return
    const target = draft
    for (const path of paths) {
      if (target.attachments.some((a) => a.path === path)) continue
      target.attachments.push({
        id: crypto.randomUUID(),
        kind: folder ? 'folder' : isImagePath(path) ? 'image' : 'file',
        path,
        name: basename(path),
      })
    }
  }

  /** Dropped items: folders and missing paths are told apart by `pathKind`. */
  async function addDropped(paths: string[]) {
    if (!editable || sending) return
    const target = draft
    for (const path of paths) {
      if (target.attachments.some((a) => a.path === path)) continue
      const id = crypto.randomUUID()
      const name = basename(path)
      let attachment: DraftAttachment
      try {
        const found = await pathKind(path)
        attachment =
          found === 'missing'
            ? { id, kind: 'file', path, name, error: 'Élément introuvable.' }
            : { id, kind: found === 'folder' ? 'folder' : isImagePath(path) ? 'image' : 'file', path, name }
      } catch (e) {
        attachment = { id, kind: 'file', path, name, error: errorMessage(e) }
      }
      // Checked again after the await: the same path may have been added meanwhile.
      if (!target.attachments.some((a) => a.path === path)) target.attachments.push(attachment)
    }
  }

  /** Saves a pasted image in the application's storage, then adds its chip. */
  async function addImage(blob: Blob, name: string) {
    // The draft this paste belongs to, even if the user switches meanwhile.
    const target = draft
    const id = crypto.randomUUID()
    const mediaType = blob.type || 'image/png'
    saving += 1
    try {
      const saved = await attachmentSave(name, mediaType, await blobToBase64(blob))
      target.attachments.push({
        id,
        kind: 'image',
        path: saved.path,
        name,
        previewUrl: URL.createObjectURL(blob),
      })
    } catch (e) {
      target.attachments.push({ id, kind: 'image', path: '', name, error: errorMessage(e) })
    } finally {
      saving -= 1
    }
  }

  function removeAttachment(id: string) {
    const target = draft
    const gone = target.attachments.find((a) => a.id === id)
    if (!gone) return
    revoke(gone)
    target.attachments = target.attachments.filter((a) => a.id !== id)
  }

  function onPaste(e: ClipboardEvent) {
    if (!editable || sending) return
    const items = Array.from(e.clipboardData?.items ?? [])
    // Plain text keeps the default paste behaviour.
    if (items.some((i) => i.kind === 'string' && i.type === 'text/plain')) return
    const image = items.find((i) => i.kind === 'file' && i.type.startsWith('image/'))
    const file = image?.getAsFile()
    if (!file) return
    e.preventDefault()
    const name = file.name || `image-${Date.now()}.${extensionFor(file.type)}`
    void addImage(file, name)
  }

  /** True when a point (physical pixels, relative to the webview) is over the composer. */
  function insideComposer(x: number, y: number): boolean {
    if (!rootEl) return false
    const r = rootEl.getBoundingClientRect()
    const scale = window.devicePixelRatio || 1
    const px = x / scale
    const py = y / scale
    return px >= r.left && px <= r.right && py >= r.top && py <= r.bottom
  }

  function onDragDrop(event: DragDropEvent) {
    if (event.type === 'leave') {
      dragging = false
      return
    }
    const inside = insideComposer(event.position.x, event.position.y)
    if (event.type === 'drop') {
      dragging = false
      if (inside) void addDropped(event.paths)
      return
    }
    dragging = inside && editable
  }

  onMount(() => {
    let disposed = false
    let unlisten: (() => void) | undefined
    try {
      void getCurrentWebview()
        .onDragDropEvent((event) => onDragDrop(event.payload))
        .then((fn) => {
          if (disposed) fn()
          else unlisten = fn
        })
        .catch(() => {})
    } catch {
      // Not running inside Tauri: there is no native drop target.
    }
    return () => {
      disposed = true
      unlisten?.()
    }
  })

  // ─── Model and effort choices ─────────────────────────────────────────────

  /** Returns the refusal (the selects revert their display), or null when accepted. */
  async function chooseModel(next: string): Promise<string | null> {
    refusal = null
    const profiles = app.model(next)?.profiles ?? []
    if (!conversation) {
      pickedModel = next
      if (pickedEffort && !profiles.some((p) => p.id === pickedEffort)) pickedEffort = null
      return null
    }
    const effort = conversation.reasoning
    const keep = effort && profiles.some((p) => p.id === effort) ? effort : undefined
    const error = await app.command(conversation.sessionId, { type: 'set_model', model: next, reasoning: keep })
    refusal = error
    return error
  }

  async function chooseEffort(profile: string | null): Promise<string | null> {
    refusal = null
    if (!conversation) {
      pickedEffort = profile
      return null
    }
    const error = await app.command(conversation.sessionId, { type: 'set_reasoning', profile })
    refusal = error
    return error
  }

  // ─── Send and cancel ──────────────────────────────────────────────────────

  /**
   * A submit whose outcome is not known yet: `text` is the draft that was sent.
   * It is cleared when its run starts; an engine refusal (`command_rejected`)
   * keeps it. `runsBefore` and `sentAt` tell the run and the refusal apart.
   */
  let awaiting = $state<{
    sessionId: string
    sentAt: number
    runsBefore: number
    text: string
  } | null>(null)

  async function send() {
    if (!canSend) {
      if (blocked) refusal = 'Retirez les pièces jointes en erreur pour envoyer.'
      return
    }
    refusal = null
    const text = draft.text.trim()
    const blocks: PromptBlock[] = []
    if (text) blocks.push({ kind: 'text', text })
    for (const a of draft.attachments) blocks.push(toBlock(a))

    const sentAt = Date.now()
    sending = true
    try {
      let target = conversation
      if (!target) {
        const created = await app.newConversation(selection ?? undefined, pickedEffort ?? undefined)
        if (!created) {
          refusal = app.lastError?.message ?? 'Impossible de créer la conversation.'
          return
        }
        // Move the local draft into the new conversation: a refusal keeps it there.
        created.draft.text = localDraft.text
        created.draft.attachments = [...localDraft.attachments]
        localDraft.text = ''
        localDraft.attachments = []
        target = created
      }
      const runsBefore = target.runs.size
      const sentText = target.draft.text
      const error = await app.command(target.sessionId, { type: 'submit', prompt: { blocks } })
      if (error) {
        refusal = error
        return
      }
      awaiting = { sessionId: target.sessionId, sentAt, runsBefore, text: sentText }
    } finally {
      sending = false
    }
  }

  // Settles the last submit: its draft goes when its run starts, and stays when
  // the engine refuses the submit.
  $effect(() => {
    const pending = awaiting
    if (!pending) return
    const conv = app.conversations.get(pending.sessionId)
    if (!conv) {
      untrack(() => (awaiting = null))
      return
    }
    const last = conv.lastRejection
    const refused = last !== null && last.command === 'submit' && last.at >= pending.sentAt
    const started = conv.runs.size > pending.runsBefore
    if (!refused && !started) return
    untrack(() => {
      awaiting = null
      if (started && !refused && conv.draft.text === pending.text) clearDraft(conv.draft)
    })
  })

  // Engine refusals of the commands this composer sends, shown near the editor.
  const REJECTION_LABELS: Record<string, string> = {
    submit: 'Envoi refusé',
    set_model: 'Changement de modèle refusé',
    set_reasoning: 'Changement d’effort refusé',
  }
  let dismissedRejectionAt = $state<number | null>(null)
  const rejection = $derived.by(() => {
    const r = conversation?.lastRejection
    if (!r || !Object.hasOwn(REJECTION_LABELS, r.command) || r.at === dismissedRejectionAt) return null
    return r
  })
  const rejectionLabel = $derived(rejection ? REJECTION_LABELS[rejection.command] : undefined)

  async function cancel() {
    if (!conversation) return
    refusal = await app.command(conversation.sessionId, { type: 'cancel' })
  }

  /** Enter sends (Cmd/Ctrl+Enter too); Shift+Enter is a newline; IME composition is left alone. */
  function onKeydown(e: KeyboardEvent) {
    if (e.key !== 'Enter' || e.shiftKey || e.altKey || e.isComposing || e.keyCode === 229) return
    e.preventDefault()
    // While a run is going, nothing is sent: the hint under the editor says so.
    if (running) return
    void send()
  }

  // ─── Editor behaviour ─────────────────────────────────────────────────────

  // Focus the editor when a conversation opens.
  $effect(() => {
    void conversation?.sessionId
    editorEl?.focus()
  })

  // Manual auto-grow where CSS field-sizing is missing (the class handles the rest).
  $effect(() => {
    void draft.text
    const el = editorEl
    if (!el || HAS_FIELD_SIZING) return
    el.style.height = 'auto'
    el.style.height = `${el.scrollHeight}px`
  })
</script>

<section
  bind:this={rootEl}
  aria-label="Zone de saisie"
  class="shrink-0 border-t bg-background px-4 pt-2 pb-3"
>
  <div class="mx-auto flex max-w-3xl flex-col gap-2">
    <ProjectStrip />

    {#if rejection && rejectionLabel}
      <div role="alert" class="flex items-start justify-between gap-2 text-sm text-destructive">
        <p>{rejectionLabel} : {rejection.reason}</p>
        <Button
          variant="ghost"
          size="icon-xs"
          aria-label="Masquer ce message"
          onclick={() => (dismissedRejectionAt = rejection.at)}
        >
          <XIcon aria-hidden="true" />
        </Button>
      </div>
    {/if}

    {#if refusal && refusal !== rejection?.reason}
      <p role="alert" class="text-sm text-destructive">{refusal}</p>
    {/if}

    <div
      class="flex flex-col gap-2 rounded-xl border bg-card p-2 shadow-xs transition-colors"
      class:border-ring={dragging}
      class:bg-accent={dragging}
    >
      {#if dragging}
        <p class="px-1 text-sm text-muted-foreground">Déposez les fichiers pour les joindre.</p>
      {/if}

      {#if draft.attachments.length > 0}
        <ul class="flex flex-wrap gap-1.5" aria-label="Pièces jointes">
          {#each draft.attachments as attachment (attachment.id)}
            <li>
              <AttachmentChip
                {attachment}
                error={chipError(attachment)}
                onRemove={() => removeAttachment(attachment.id)}
              />
            </li>
          {/each}
        </ul>
      {/if}

      {#if saving > 0}
        <p class="px-1 text-xs text-muted-foreground" role="status">Enregistrement de la pièce jointe…</p>
      {/if}

      <Textarea
        bind:ref={editorEl}
        rows={1}
        value={draft.text}
        oninput={(e) => (draft.text = e.currentTarget.value)}
        onkeydown={onKeydown}
        onpaste={onPaste}
        disabled={!editable}
        readonly={sending}
        {placeholder}
        aria-label="Message"
        class="max-h-[40vh] min-h-10 resize-none overflow-y-auto border-0 bg-transparent px-1 py-1 shadow-none focus-visible:ring-0 dark:bg-transparent"
      />

      <div class="flex flex-wrap items-center gap-1">
        <AttachMenu
          disabled={!editable || sending}
          onPick={(paths, kind) => addPicked(paths, kind === 'folder')}
          onError={(message) => (refusal = message)}
        />
        <ModelSelect value={selection} models={app.models} disabled={!editable} onChange={chooseModel} />
        {#if choice && choice.profiles.length > 0}
          <EffortSelect
            value={effortValue}
            effective={effortEffective}
            profiles={choice.profiles}
            defaultProfile={choice.default_profile}
            disabled={!editable}
            onChange={chooseEffort}
          />
        {/if}
        {#if pendingText}
          <span class="text-xs text-muted-foreground" role="status">{pendingText}</span>
        {/if}

        <div class="ml-auto flex items-center gap-1">
          {#if running}
            <Button variant="destructive" size="sm" onclick={cancel}>
              <SquareIcon aria-hidden="true" />
              Arrêter
            </Button>
          {:else}
            <Button
              size="icon"
              aria-label="Envoyer le message"
              title="Envoyer (Entrée)"
              disabled={!canSend}
              onclick={send}
            >
              <ArrowUpIcon aria-hidden="true" />
            </Button>
          {/if}
        </div>
      </div>
    </div>

    {#if running}
      <p class="px-1 text-xs text-muted-foreground" role="status">
        Une génération est en cours : le brouillon sera conservé.
      </p>
    {/if}
  </div>
</section>
