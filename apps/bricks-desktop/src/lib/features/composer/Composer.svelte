<script lang="ts">
  import { onMount, tick, untrack } from 'svelte'
  import { getCurrentWebview } from '@tauri-apps/api/webview'
  import type { DragDropEvent } from '@tauri-apps/api/webview'
  import { Button } from '$lib/components/ui/button'
  import { Textarea } from '$lib/components/ui/textarea'
  import * as Tooltip from '$lib/components/ui/tooltip'
  import { app } from '$lib/state/app.svelte'
  import { ui } from '$lib/state/ui.svelte'
  import type { Conversation, Draft, DraftAttachment } from '$lib/state/conversation.svelte'
  import {
    appQuit,
    attachmentSave,
    errorMessage,
    pathKind,
    slashParse,
  } from '$lib/ipc/client'
  import type { SlashCommandInfo } from '$lib/ipc/bindings/SlashCommandInfo'
  import type { PromptBlock } from '$lib/ipc/engine'
  import ArrowUpIcon from '@lucide/svelte/icons/arrow-up'
  import SquareIcon from '@lucide/svelte/icons/square'
  import XIcon from '@lucide/svelte/icons/x'
  import SparklesIcon from '@lucide/svelte/icons/sparkles'
  import ProjectStrip from './ProjectStrip.svelte'
  import AttachMenu from './AttachMenu.svelte'
  import AttachmentChip from './AttachmentChip.svelte'
  import ModelSelect from './ModelSelect.svelte'
  import ContextGauge from '$lib/features/context/ContextGauge.svelte'
  import EffortSelect from './EffortSelect.svelte'
  import CommandPalette from '$lib/features/commands/CommandPalette.svelte'
  import QuitDialog from '$lib/features/commands/QuitDialog.svelte'
  import MentionPalette from './MentionPalette.svelte'
  import SkillPicker from './SkillPicker.svelte'
  import { selectionAfterRun, toggleSkill } from './skills'
  import { MentionFiles } from './mention-files.svelte'
  import { mentionAt, mentionKind, projectEntryPath, removeMention, type MentionToken } from './mentions'
  import type { ProjectFileEntry } from '$lib/ipc/bindings/ProjectFileEntry'
  import { slashRegistry } from '$lib/features/commands/slash-registry.svelte'
  import {
    completeLine,
    filterCommands,
    isExactName,
    moveIndex,
    paletteQuery,
  } from '$lib/features/commands/palette'
  import {
    HistoryCursor,
    shouldRecallNewer,
    shouldRecallOlder,
  } from '$lib/features/commands/history'
  import {
    runSlashLine,
    type AttachKind,
    type SlashHost,
    type SlashOutcome,
  } from '$lib/features/commands/dispatch'
  import { basename, blobToBase64, extensionFor, isImagePath } from './attachments'

  let { conversation }: { conversation: Conversation | null } = $props()

  const NO_IMAGE_MESSAGE = 'Ce modèle n’accepte pas les images : retirez cette pièce jointe.'
  // Browsers without CSS field-sizing get the manual height fallback below.
  const HAS_FIELD_SIZING = typeof CSS !== 'undefined' && CSS.supports('field-sizing', 'content')
  const uid = $props.id()
  const paletteId = `${uid}-palette`

  const project = $derived(app.currentProject)

  // ─── Draft ────────────────────────────────────────────────────────────────
  // An open conversation keeps its own draft (it survives switches). Without
  // one, the project's draft (kept by the app state, one per project) is used
  // until the first valid send creates the conversation.
  let localDraft = $state<Draft>({ text: '', attachments: [], skills: [] })
  const draft: Draft = $derived(
    conversation ? conversation.draft : project ? app.draftFor(project.id) : localDraft,
  )

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
  // Set by `/model`, `/effort` (through `ui.picker`) to open the selects.
  let modelOpen = $state(false)
  let effortOpen = $state(false)
  /** The skills choice of the `+` menu. */
  let skillsOpen = $state(false)

  const hasModels = $derived(app.models.length > 0)
  const selection: string | null = $derived(
    conversation
      ? conversation.model
      : project
        ? app.draftModel(project.id)
        : (pickedModel ?? app.models[0]?.selection ?? null),
  )
  const choice = $derived(app.model(selection))
  const draftEffort: string | null = $derived(project ? app.draftReasoning(project.id) : pickedEffort)
  const effortValue: string | null = $derived(conversation ? conversation.reasoning : draftEffort)
  const effortEffective: string | null = $derived(
    conversation ? conversation.effectiveReasoning : (draftEffort ?? choice?.default_profile ?? null),
  )
  const supportsImage = $derived(choice?.input.includes('image') ?? false)

  // ─── State flags ──────────────────────────────────────────────────────────
  const editable = $derived(project !== null && !project.missing && hasModels)
  const running = $derived(conversation?.running ?? false)

  let saving = $state(0)
  let sending = $state(false)
  let dragging = $state(false)
  let refusal = $state<string | null>(null)
  let rootEl = $state<HTMLElement | null>(null)
  let editorEl = $state<HTMLTextAreaElement | null>(null)

  /** Why an attachment cannot be sent, or `null` when it can. */
  function chipError(a: DraftAttachment): string | null {
    if (a.error) return a.error
    if (a.kind === 'image' && !supportsImage) return NO_IMAGE_MESSAGE
    return null
  }

  const blocked = $derived(draft.attachments.some((a) => chipError(a) !== null))
  const hasContent = $derived(draft.text.trim() !== '' || draft.attachments.length > 0)
  const canSend = $derived(editable && !running && !sending && saving === 0 && !blocked && hasContent)
  /** A `/` line is run by the composer, never sent as a prompt (leading spaces do not count). */
  const isSlashLine = $derived(draft.text.trimStart().startsWith('/'))
  const slashReady = $derived(isSlashLine && editable && !sending && saving === 0)

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
    return 'Quelque chose à dire ?'
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

  /** Attaches a path named by `/file`, `/folder` or `/image` (the path is checked first). */
  async function attachByPath(kind: AttachKind, path: string): Promise<string | null> {
    if (!editable) return 'Ouvrez un projet et configurez un modèle pour joindre un élément.'
    let found: 'file' | 'folder' | 'missing'
    try {
      found = await pathKind(path)
    } catch (e) {
      return errorMessage(e)
    }
    if (found === 'missing') return `Élément introuvable : ${path}`
    if (kind === 'folder' && found !== 'folder') return `« ${path} » n’est pas un dossier.`
    if (kind !== 'folder' && found === 'folder') return `« ${path} » est un dossier : utilisez /folder.`
    if (kind === 'image' && !isImagePath(path)) {
      return `« ${path} » n’est pas une image (png, jpeg, gif, webp).`
    }
    const target = draft
    if (target.attachments.some((a) => a.path === path)) return null
    target.attachments.push({
      id: crypto.randomUUID(),
      kind: found === 'folder' ? 'folder' : isImagePath(path) ? 'image' : 'file',
      path,
      name: basename(path),
    })
    return null
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
    void slashRegistry.load()
    return () => {
      disposed = true
      unlisten?.()
      files.dispose()
    }
  })

  // ─── Model and effort choices ─────────────────────────────────────────────

  /** Model choice without an open conversation: kept for the first send. */
  function applyLocalModel(next: string, reasoning: string | null): string | null {
    const target = app.model(next)
    if (!target) return `Modèle inconnu : ${next}.`
    const known = (profile: string | null) =>
      profile !== null && target.profiles.some((p) => p.id === profile)
    if (reasoning !== null && !known(reasoning)) {
      return `Profil d’effort inconnu pour ${target.name} : ${reasoning}.`
    }
    pickedModel = next
    pickedEffort = reasoning ?? (known(draftEffort) ? draftEffort : null)
    if (project) app.setDraftModel(project.id, next, pickedEffort)
    return null
  }

  /** Effort choice without an open conversation: kept for the first send. */
  function applyLocalEffort(profile: string | null): string | null {
    if (profile !== null && !(choice?.profiles.some((p) => p.id === profile) ?? false)) {
      return `Profil d’effort inconnu : ${profile}.`
    }
    pickedEffort = profile
    if (project && selection) app.setDraftModel(project.id, selection, profile)
    return null
  }

  /** Returns the refusal (the selects revert their display), or null when accepted. */
  async function chooseModel(next: string): Promise<string | null> {
    refusal = null
    if (!conversation) {
      refusal = applyLocalModel(next, null)
      return refusal
    }
    const profiles = app.model(next)?.profiles ?? []
    const effort = conversation.reasoning
    const keep = effort && profiles.some((p) => p.id === effort) ? effort : undefined
    const error = await app.command(conversation.sessionId, { type: 'set_model', model: next, reasoning: keep })
    refusal = error
    return error
  }

  async function chooseEffort(profile: string | null): Promise<string | null> {
    refusal = null
    if (!conversation) {
      refusal = applyLocalEffort(profile)
      return refusal
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
   * `restore` is the draft kept aside while browsing history (put back once
   * the recalled entry is sent); `skill` is the skill of a `/skill` line.
   */
  let awaiting = $state<{
    sessionId: string
    sentAt: number
    runsBefore: number
    text: string
    restore: string | null
    skill: string | null
    /** Skills chosen for this message: no longer chosen once its run starts. */
    skills: string[]
  } | null>(null)

  /** Input history browsing: the draft kept aside while an older prompt is shown. */
  const cursor = new HistoryCursor<Draft>()

  /** The `/compact` sent from here: its progress and result show near the composer. */
  let compactWatch = $state<{ sessionId: string; sentAt: number } | null>(null)

  /** Active runs listed by `/quit`; `null` when the confirmation is closed. */
  let quitRuns = $state<string[] | null>(null)

  /**
   * The open conversation, or the single new one created for the project's
   * draft (shared by concurrent callers; the app state copies the draft into
   * it). `createdFor` names the project whose draft is cleared once the first
   * submit is accepted.
   */
  let createdFor: string | null = null
  async function ensureConversation(): Promise<{ conversation: Conversation } | { error: string }> {
    if (conversation) return { conversation }
    const projectId = project?.id ?? null
    const created = await app.startConversation(selection ?? undefined, effortValue ?? undefined)
    if (!created) return { error: app.lastError?.message ?? 'Impossible de créer la conversation.' }
    createdFor = projectId
    return { conversation: created }
  }

  /** After the first accepted submit of a new conversation, the project draft is spent. */
  function spendProjectDraft() {
    if (createdFor) app.clearProjectDraft(createdFor)
    createdFor = null
  }

  async function send() {
    if (isSlashLine) return runSlashDraft()
    if (!canSend) {
      if (blocked) refusal = 'Retirez les pièces jointes en erreur pour envoyer.'
      return
    }
    refusal = null
    const text = draft.text.trim()
    const blocks: PromptBlock[] = []
    if (text) blocks.push({ kind: 'text', text })
    for (const a of draft.attachments) blocks.push(toBlock(a))
    const skills = [...draft.skills]

    const sentAt = Date.now()
    sending = true
    try {
      const ensured = await ensureConversation()
      if ('error' in ensured) {
        refusal = ensured.error
        return
      }
      const target = ensured.conversation
      const runsBefore = target.runs.size
      const sentText = target.draft.text
      const prompt = skills.length > 0 ? { blocks, skills } : { blocks }
      const error = await app.command(target.sessionId, { type: 'submit', prompt })
      if (error) {
        refusal = error
        return
      }
      spendProjectDraft()
      // A recalled entry was sent: the draft kept aside comes back once its run starts.
      const restore = cursor.end()
      awaiting = {
        sessionId: target.sessionId,
        sentAt,
        runsBefore,
        text: sentText,
        restore: restore?.text ?? null,
        skill: null,
        skills,
      }
    } finally {
      sending = false
    }
  }

  // Settles the last submit: its draft goes when its run starts, and stays when
  // the engine refuses the submit or a `/skill` cannot be activated.
  $effect(() => {
    const pending = awaiting
    if (!pending) return
    const conv = app.conversations.get(pending.sessionId)
    if (!conv) {
      untrack(() => (awaiting = null))
      return
    }
    const last = conv.lastRejection
    const refusedByEngine = last !== null && last.command === 'submit' && last.at >= pending.sentAt
    const skill = conv.lastSkill
    const failed =
      pending.skill !== null && skill !== null && !skill.ok && skill.at >= pending.sentAt ? skill : null
    // A chosen skill the engine could not activate: its choice stays for the next message.
    const chosenRefused =
      skill !== null && !skill.ok && skill.at >= pending.sentAt && pending.skills.includes(skill.name)
    const started = conv.runs.size > pending.runsBefore
    if (!refusedByEngine && !failed && !started) return
    untrack(() => {
      awaiting = null
      if (failed) {
        refusal = `Skill « ${failed.name} » non activé : ${failed.reason ?? 'raison inconnue'}. Rien n’a été envoyé.`
        return
      }
      if (refusedByEngine) return
      if (!chosenRefused) conv.draft.skills = selectionAfterRun(conv.draft.skills, pending.skills)
      if (pending.restore !== null) {
        for (const a of conv.draft.attachments) revoke(a)
        conv.draft.text = pending.restore
        conv.draft.attachments = []
      } else if (conv.draft.text === pending.text) {
        clearDraft(conv.draft)
      }
    })
  })

  // Engine refusals of the commands this composer sends, shown near the editor.
  const REJECTION_LABELS: Record<string, string> = {
    submit: 'Envoi refusé',
    set_model: 'Changement de modèle refusé',
    set_reasoning: 'Changement d’effort refusé',
    compact: 'Compaction refusée',
  }
  let dismissedRejectionAt = $state<number | null>(null)
  const rejection = $derived.by(() => {
    const r = conversation?.lastRejection
    if (!r || r.at === dismissedRejectionAt) return null
    return r
  })
  const rejectionLabel = $derived(
    rejection ? (REJECTION_LABELS[rejection.command] ?? 'Commande refusée') : null,
  )

  /** Progress and result of the `/compact` sent from this composer. */
  const compactStatus = $derived.by(() => {
    const w = compactWatch
    if (!w || conversation?.sessionId !== w.sessionId) return null
    const conv = app.conversations.get(w.sessionId)
    const done = conv?.lastCompaction
    if (done && done.at >= w.sentAt) return `Compaction : ${done.outcome}`
    const refusedNow = conv?.lastRejection
    if (refusedNow && refusedNow.command === 'compact' && refusedNow.at >= w.sentAt) return null
    return 'Compaction en cours…'
  })

  async function cancel() {
    if (!conversation) return
    refusal = await app.command(conversation.sessionId, { type: 'cancel' })
  }

  // ─── `/` lines ────────────────────────────────────────────────────────────

  /** Runs the `/` line of the draft. The engine parses it; nothing is sent as a prompt. */
  async function runSlashDraft() {
    if (!editable || sending || saving > 0) return
    refusal = null
    const origin = draft
    const sentText = origin.text
    sending = true
    try {
      const outcome = await runSlashLine(sentText.trim(), slashHost)
      settleSlash(outcome, sentText, origin)
    } finally {
      sending = false
    }
  }

  /** Clears the typed line, keeps it on a refusal, or starts watching what the engine answers. */
  function settleSlash(outcome: SlashOutcome, sentText: string, origin: Draft) {
    switch (outcome.kind) {
      case 'refused':
        refusal = outcome.message
        return
      case 'done': {
        // The line is consumed: a draft kept aside while browsing history comes back.
        origin.text = cursor.end()?.text ?? ''
        return
      }
      case 'sent': {
        const { sessionId, sentAt, runsBefore, watch } = outcome
        const restore = cursor.end()?.text ?? null
        if (watch.kind === 'submit' || watch.kind === 'skill') {
          spendProjectDraft()
          awaiting = {
            sessionId,
            sentAt,
            runsBefore,
            text: sentText,
            restore,
            skill: watch.kind === 'skill' ? watch.name : null,
            skills: [],
          }
          return
        }
        if (watch.kind === 'compact') compactWatch = { sessionId, sentAt }
        const target = app.conversations.get(sessionId)?.draft ?? origin
        target.text = restore ?? ''
      }
    }
  }

  /** `/quit`: asks first when a run is active, then closes the application. */
  async function requestQuit(): Promise<string | null> {
    const active = [...app.conversations.values()]
      .filter((c) => c.running)
      .map((c) => c.title || 'Conversation sans titre')
    if (active.length > 0) {
      quitRuns = active
      return null
    }
    try {
      await appQuit()
      return null
    } catch (e) {
      return errorMessage(e)
    }
  }

  /** What the `/` dispatcher needs from this composer. */
  const slashHost: SlashHost = {
    async parse(line) {
      try {
        return await slashParse(line)
      } catch (e) {
        throw new Error(errorMessage(e))
      }
    },
    currentSession: () => conversation?.sessionId ?? null,
    async ensureSession() {
      if (conversation) return { sessionId: conversation.sessionId }
      if (!project) return { error: 'Ouvrez un projet pour utiliser cette commande.' }
      if (project.missing) return { error: 'Le dossier du projet est introuvable.' }
      if (!hasModels) return { error: 'Aucun modèle configuré.' }
      const ensured = await ensureConversation()
      return 'error' in ensured ? ensured : { sessionId: ensured.conversation.sessionId }
    },
    mark: (sessionId) => ({
      sentAt: Date.now(),
      runsBefore: app.conversations.get(sessionId)?.runs.size ?? 0,
    }),
    send: (sessionId, command, display) => app.command(sessionId, command, display ?? null),
    async setModel(model, reasoning) {
      refusal = null
      return applyLocalModel(model, reasoning)
    },
    async setEffort(profile) {
      refusal = null
      return applyLocalEffort(profile)
    },
    openPicker: (picker) => ui.openPicker(picker),
    open: (view, args) => ui.open(view, args),
    attachmentBlocks() {
      if (saving > 0) return { error: 'Attendez la fin de l’enregistrement de la pièce jointe.' }
      if (blocked) return { error: 'Retirez les pièces jointes en erreur pour envoyer.' }
      return { blocks: draft.attachments.map(toBlock) }
    },
    resume: (sessionId) => app.resumeSession(sessionId),
    showChanges() {
      // The right panel's visibility is owned by the shell: see the report.
      app.rightView = 'changes'
    },
    attach: attachByPath,
    quit: requestQuit,
  }

  // ─── Skills for the next message ──────────────────────────────────────────

  function openSkills() {
    refusal = null
    skillsOpen = true
  }

  function closeSkills() {
    skillsOpen = false
    editorEl?.focus()
  }

  /** Adds a skill to the next message, or removes it when already chosen (no file is touched). */
  function chooseSkill(name: string) {
    draft.skills = toggleSkill(draft.skills, name)
  }

  function removeSkill(name: string) {
    draft.skills = draft.skills.filter((n) => n !== name)
  }

  // ─── Palette `/` ──────────────────────────────────────────────────────────

  /** The palette hides after Esc until the draft changes. */
  let dismissedFor = $state<string | null>(null)
  let paletteIndex = $state(0)

  const paletteName = $derived(paletteQuery(draft.text.trimStart()))
  const paletteItems: SlashCommandInfo[] = $derived(
    slashRegistry.commands ? filterCommands(slashRegistry.commands, paletteName ?? '') : [],
  )
  const paletteOpen = $derived(paletteName !== null && editable && dismissedFor !== draft.text)
  const activeIndex = $derived(Math.min(paletteIndex, Math.max(paletteItems.length - 1, 0)))
  const activeCommand: SlashCommandInfo | undefined = $derived(paletteItems[activeIndex])
  const paletteListShown = $derived(
    paletteOpen && slashRegistry.state.status === 'ready' && paletteItems.length > 0,
  )

  // ─── `@` mentions ────────────────────────────────────────────────────────

  const mentionId = `${uid}-mention`
  const files = new MentionFiles()
  /** Caret position in the editor, kept in step with the text. */
  let caret = $state(0)
  let mentionIndex = $state(0)
  /** The `@` popover hides after Esc until the draft changes. */
  let mentionDismissedFor = $state<string | null>(null)

  const mention: MentionToken | null = $derived(editable ? mentionAt(draft.text, caret) : null)
  const mentionOpen = $derived(mention !== null && !paletteOpen && mentionDismissedFor !== draft.text)
  const mentionActiveIndex = $derived(Math.min(mentionIndex, Math.max(files.items.length - 1, 0)))
  const mentionActive: ProjectFileEntry | undefined = $derived(files.items[mentionActiveIndex])

  /** The popup the editor points at: the command palette wins over `@`. */
  const popupId = $derived(paletteOpen ? paletteId : mentionOpen ? mentionId : null)
  const popupShown = $derived(paletteOpen ? paletteListShown : mentionOpen && files.items.length > 0)
  const popupActive = $derived(paletteOpen ? activeIndex : mentionActiveIndex)

  /** Puts the caret at `pos` once the text is rendered. */
  function moveCaret(pos: number) {
    caret = pos
    void tick().then(() => {
      const el = editorEl
      if (el) el.setSelectionRange(pos, pos)
    })
  }

  /** Puts the caret at the end once the new text is rendered. */
  function caretToEnd() {
    moveCaret(draft.text.length)
  }

  /** Keeps the caret state in step with the editor (typing, arrows, clicks). */
  function syncCaret() {
    const el = editorEl
    if (el) caret = el.selectionStart
  }

  /** Completes the name of a command (Tab, Enter on an incomplete name, or a click). */
  function completeWith(cmd: SlashCommandInfo) {
    draft.text = completeLine(cmd)
    paletteIndex = 0
    dismissedFor = null
    caretToEnd()
    editorEl?.focus()
  }

  /** Keys handled while the palette is open. Returns true when the key was used. */
  function onPaletteKey(e: KeyboardEvent): boolean {
    const count = paletteItems.length
    switch (e.key) {
      case 'ArrowDown':
      case 'ArrowUp':
        if (count === 0) return false
        e.preventDefault()
        paletteIndex = moveIndex(activeIndex, e.key === 'ArrowDown' ? 1 : -1, count)
        return true
      case 'Tab': {
        const cmd = activeCommand
        if (!cmd || e.shiftKey) return false
        e.preventDefault()
        completeWith(cmd)
        return true
      }
      case 'Enter': {
        const cmd = activeCommand
        // A complete name runs the command (the normal Enter path); an incomplete one is completed first.
        if (!cmd || e.shiftKey || e.altKey || e.ctrlKey || e.metaKey) return false
        if (isExactName(paletteName ?? '', cmd)) return false
        e.preventDefault()
        completeWith(cmd)
        return true
      }
      case 'Escape':
        e.preventDefault()
        e.stopPropagation()
        dismissedFor = draft.text
        return true
      default:
        return false
    }
  }

  /** Rebuilds the project's file index (« Actualiser », Ctrl+R). */
  function refreshMentions() {
    const pid = project?.id
    if (!pid || !mention) return
    files.refresh(pid, mention.query)
  }

  /**
   * Attaches a project entry (folder, image or file) and removes its `@` token
   * from the text. An entry that has vanished since the listing becomes an error chip.
   */
  async function attachEntry(entry: ProjectFileEntry) {
    const token = mention
    const root = app.currentProject
    if (!token || !root || !editable || sending) return
    const target = draft
    const next = removeMention(target.text, token)
    target.text = next.text
    mentionIndex = 0
    moveCaret(next.caret)
    const path = projectEntryPath(root.path, entry.path)
    if (target.attachments.some((a) => a.path === path)) return
    const id = crypto.randomUUID()
    const name = basename(path)
    let attachment: DraftAttachment
    try {
      const found = await pathKind(path)
      attachment =
        found === 'missing'
          ? { id, kind: 'file', path, name, error: 'Élément introuvable.' }
          : { id, kind: mentionKind(path, found), path, name }
    } catch (e) {
      attachment = { id, kind: 'file', path, name, error: errorMessage(e) }
    }
    // Checked again after the await: the same path may have been added meanwhile.
    if (!target.attachments.some((a) => a.path === path)) target.attachments.push(attachment)
  }

  /** Keys handled while the `@` popover is open. Returns true when the key was used. */
  function onMentionKey(e: KeyboardEvent): boolean {
    if (e.ctrlKey && e.key.toLowerCase() === 'r') {
      e.preventDefault()
      refreshMentions()
      return true
    }
    const count = files.items.length
    switch (e.key) {
      case 'ArrowDown':
      case 'ArrowUp':
        if (count === 0) return false
        e.preventDefault()
        mentionIndex = moveIndex(mentionActiveIndex, e.key === 'ArrowDown' ? 1 : -1, count)
        return true
      case 'Tab': {
        const entry = mentionActive
        if (!entry || e.shiftKey) return false
        e.preventDefault()
        void attachEntry(entry)
        return true
      }
      case 'Enter': {
        const entry = mentionActive
        if (!entry || e.shiftKey || e.altKey || e.ctrlKey || e.metaKey) return false
        e.preventDefault()
        void attachEntry(entry)
        return true
      }
      case 'Escape':
        e.preventDefault()
        e.stopPropagation()
        mentionDismissedFor = draft.text
        return true
      default:
        return false
    }
  }

  // ─── Input history ────────────────────────────────────────────────────────

  /** Earlier prompts of the open conversation, oldest first. */
  function historyEntries(): string[] {
    const out: string[] = []
    for (const item of conversation?.items ?? []) {
      if (item.kind === 'user' && item.text.trim() !== '') out.push(item.text)
    }
    // A send whose run has not started yet is not in the timeline.
    if (awaiting && conversation && awaiting.sessionId === conversation.sessionId) {
      if (awaiting.text.trim() !== '') out.push(awaiting.text)
    }
    return out
  }

  /** ↑ / ↓ recall earlier prompts. Returns true when the key was used. */
  function recall(e: KeyboardEvent): boolean {
    const el = editorEl
    if (!el) return false
    const caret = {
      text: draft.text,
      selectionStart: el.selectionStart,
      selectionEnd: el.selectionEnd,
      browsing: cursor.browsing,
    }
    const next =
      e.key === 'ArrowUp'
        ? shouldRecallOlder(caret)
          ? cursor.older(historyEntries(), draft.text, draft)
          : null
        : shouldRecallNewer(caret)
          ? cursor.newer(historyEntries())
          : null
    if (next === null) return false
    e.preventDefault()
    draft.text = next
    caretToEnd()
    return true
  }

  // ─── Keyboard and editor ──────────────────────────────────────────────────

  function onInput(e: Event & { currentTarget: HTMLTextAreaElement }) {
    const value = e.currentTarget.value
    draft.text = value
    paletteIndex = 0
    mentionIndex = 0
    caret = e.currentTarget.selectionStart
    // Typing `/` opens the palette: a list that failed to load is asked again then.
    if (value === '/') void slashRegistry.load()
  }

  /** Enter sends (Cmd/Ctrl+Enter too); Shift+Enter is a newline; IME composition is left alone. */
  function onKeydown(e: KeyboardEvent) {
    if (e.isComposing || e.keyCode === 229) return
    if (paletteOpen && onPaletteKey(e)) return
    if (mentionOpen && onMentionKey(e)) return
    if (e.key === 'ArrowUp' || e.key === 'ArrowDown') {
      if (!e.altKey && !e.ctrlKey && !e.metaKey && !e.shiftKey) recall(e)
      return
    }
    if (e.key !== 'Enter' || e.shiftKey || e.altKey) return
    e.preventDefault()
    // While a run is going, a prompt waits; a `/` line still runs (its command decides).
    if (running && !isSlashLine) return
    void send()
  }

  // Focus the editor when a conversation opens; a recalled entry is kept aside
  // in the conversation it came from and put back when the conversation changes.
  $effect(() => {
    void conversation?.sessionId
    untrack(() => {
      const saved = cursor.end()
      if (saved) saved.owner.text = saved.text
      // A new conversation's text: the caret is at its end until the editor reports otherwise.
      caret = draft.text.length
    })
    editorEl?.focus()
  })

  // « Nouvelle conversation » (any entry point) asks the composer for focus.
  $effect(() => {
    if (app.composerFocus === 0) return
    editorEl?.focus()
  })

  // Searches the project files for the text after `@` while the popover is open.
  $effect(() => {
    const token = mentionOpen ? mention : null
    const pid = project?.id ?? null
    if (token === null || pid === null) return
    untrack(() => files.search(pid, token.query))
  })

  // `/model` and `/effort` open their select (the request is consumed here).
  $effect(() => {
    const requested = ui.picker
    if (requested === null) return
    untrack(() => {
      ui.picker = null
      if (!editable) {
        refusal = 'Choisissez un projet et un modèle avant de changer ce réglage.'
        return
      }
      if (requested === 'model') {
        modelOpen = true
        return
      }
      if (!choice || choice.profiles.length === 0) {
        refusal = 'Ce modèle n’a pas de profil d’effort configuré.'
        return
      }
      effortOpen = true
    })
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

    {#if compactStatus}
      <p class="px-1 text-xs text-muted-foreground" role="status">{compactStatus}</p>
    {/if}

    <div
      class="relative mt-1 flex flex-col gap-2 rounded-2xl border bg-card p-2 shadow-xs transition-colors focus-within:border-ring/60 focus-within:ring-2 focus-within:ring-ring/20"
      class:border-ring={dragging}
      class:bg-accent={dragging}
    >
      {#if paletteOpen}
        <CommandPalette
          id={paletteId}
          items={paletteItems}
          activeIndex={activeIndex}
          status={slashRegistry.state.status}
          error={slashRegistry.state.status === 'error' ? slashRegistry.state.message : ''}
          query={paletteName ?? ''}
          onPick={completeWith}
          onHover={(index) => (paletteIndex = index)}
        />
      {/if}

      {#if mentionOpen}
        <MentionPalette
          id={mentionId}
          items={files.items}
          activeIndex={mentionActiveIndex}
          status={files.status}
          error={files.error}
          query={mention?.query ?? ''}
          onPick={(entry) => void attachEntry(entry)}
          onHover={(index) => (mentionIndex = index)}
          onRefresh={refreshMentions}
        />
      {/if}

      {#if skillsOpen}
        <SkillPicker
          conversation={conversation}
          projectId={project?.id ?? null}
          selected={draft.skills}
          onToggle={chooseSkill}
          onClose={closeSkills}
        />
      {/if}

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

      {#if draft.skills.length > 0}
        <ul class="flex flex-wrap gap-1.5" aria-label="Skills pour le prochain message">
          {#each draft.skills as name (name)}
            <li class="inline-flex max-w-full items-center gap-1.5 rounded-md border border-transparent bg-muted/60 py-1 pr-1 pl-1.5 text-sm">
              <SparklesIcon class="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
              <span class="min-w-0 max-w-56 truncate font-mono text-xs" title={name}>{name}</span>
              <Button variant="ghost" size="icon-xs" aria-label={`Retirer le skill ${name}`} onclick={() => removeSkill(name)}>
                <XIcon aria-hidden="true" />
              </Button>
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
        oninput={onInput}
        onkeydown={onKeydown}
        onpaste={onPaste}
        disabled={!editable}
        readonly={sending}
        {placeholder}
        aria-label="Message"
        role="combobox"
        aria-autocomplete="list"
        aria-haspopup="listbox"
        aria-expanded={paletteOpen || mentionOpen}
        aria-controls={popupShown && popupId ? popupId : undefined}
        aria-activedescendant={popupShown && popupId ? `${popupId}-opt-${popupActive}` : undefined}
        onkeyup={syncCaret}
        onclick={syncCaret}
        onselect={syncCaret}
        dir="auto"
        class="max-h-[40vh] min-h-0 resize-none overflow-y-auto border-0 bg-transparent px-2 py-2 leading-5 text-start shadow-none focus-visible:ring-0 dark:bg-transparent"
      />

      <div class="flex flex-wrap items-center gap-1">
        <AttachMenu
          disabled={!editable || sending}
          onPick={(paths, kind) => addPicked(paths, kind === 'folder')}
          onError={(message) => (refusal = message)}
          onSkills={openSkills}
        />
        <ContextGauge {conversation} />
        <ModelSelect
          bind:open={modelOpen}
          value={selection}
          models={app.models}
          disabled={!editable}
          onChange={chooseModel}
        />
        {#if choice && choice.profiles.length > 0}
          <EffortSelect
            bind:open={effortOpen}
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
          {#if running && !isSlashLine}
            <Button variant="destructive" size="sm" onclick={cancel}>
              <SquareIcon aria-hidden="true" />
              Arrêter
            </Button>
          {:else}
            <Tooltip.Root>
              <Tooltip.Trigger>
                {#snippet child({ props })}
                  <Button
                    {...props}
                    size="icon"
                    aria-label="Envoyer le message"
                    disabled={!canSend && !slashReady}
                    onclick={send}
                  >
                    <ArrowUpIcon aria-hidden="true" />
                  </Button>
                {/snippet}
              </Tooltip.Trigger>
              <Tooltip.Content side="top">Envoyer</Tooltip.Content>
            </Tooltip.Root>
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

<QuitDialog runs={quitRuns} onClose={() => (quitRuns = null)} />
