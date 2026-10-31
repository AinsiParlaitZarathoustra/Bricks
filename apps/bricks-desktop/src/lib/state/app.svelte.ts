// Application state: configuration, projects, conversations, the open
// sessions and the single event subscription that feeds them.
// Owner: orchestrator. Components read `app` and call its methods; they
// never call the provider or recompute engine state themselves.

import { open as openDialog } from '@tauri-apps/plugin-dialog'
import { SvelteMap, SvelteSet } from 'svelte/reactivity'
import * as ipc from '$lib/ipc/client'
import type {
  AppInfo,
  DesktopEvent,
  Preferences,
  Project,
  SessionItem,
  SessionSearchHit,
} from '$lib/ipc/client'
import type {
  AskTimer,
  Command,
  Envelope,
  ModelChoice,
  PromptBlock,
  QuestionAnswer,
  SessionPolicy,
} from '$lib/ipc/engine'
import { Conversation, type Draft, type UserItem } from './conversation.svelte'
import { defaultDiagramSettings } from '$lib/features/diagrams/settings'
import { reader } from './reader.svelte'

export const DEFAULT_PREFERENCES: Preferences = {
  theme: 'system',
  left_open: true,
  right_open: true,
  left_width: 260,
  right_width: 280,
  terminal_open: false,
  terminal_height: 240,
  last_project_id: null,
  last_session_id: null,
  colors: { light: { accent: null, background: null, panel: null }, dark: { accent: null, background: null, panel: null } },
  terminal_ratio: null,
  diagrams: defaultDiagramSettings(),
  app_icon: 'aligned',
  ask_timer: { enabled: false, mode: 'smart', seconds: 300 },
  memory_enabled: false,
  computer_use_enabled: false,
  keep_awake: false,
}

/** An IPC refusal because a project's folder is missing. */
function isFolderMissing(e: unknown): boolean {
  return typeof e === 'object' && e !== null && 'code' in e && e.code === 'folder_missing'
}

export type RightView = 'context' | 'changes' | 'reader'

/**
 * How a conversation that has just opened takes the view: `always` (the user
 * asked for it), `draft` (a new conversation: only while the user is still on
 * the draft of its project), `never` (kept in the background).
 */
type Reveal = 'always' | 'draft' | 'never'

/** A project whose folder is missing, and what waits for it to be found again. */
export interface FolderRequest {
  projectId: string
  /** The action asked for, run again once the folder is chosen. */
  retry: (() => Promise<unknown>) | null
}

interface OpenOptions {
  /** The project draft a new conversation takes over (copied before it is shown). */
  draftFrom?: Draft
  reveal?: Reveal
}

class AppStore {
  ready = $state(false)
  /** Fatal start-up error (the bridge itself failed). */
  startError = $state<string | null>(null)

  info = $state<AppInfo | null>(null)
  models = $state<ModelChoice[]>([])
  prefs = $state<Preferences>({ ...DEFAULT_PREFERENCES })

  /** The registered projects, then the built-in one of the conversations without a project (`chats`). */
  projects = $state<Project[]>([])
  sessionsByProject = new SvelteMap<string, SessionItem[]>()
  /** Projects whose conversations are folded away in the left panel. */
  collapsedProjects = new SvelteSet<string>()
  conversations = new SvelteMap<string, Conversation>()

  currentProjectId = $state<string | null>(null)
  currentSessionId = $state<string | null>(null)
  /** Right panel view requested from the top bar. */
  rightView = $state<RightView>('context')

  /** Set while the folder of a project must be found again before an action goes on. */
  folderRequest = $state<FolderRequest | null>(null)

  /** Last user-facing error of an action (shown as a toast by the UI). */
  lastError = $state<{ message: string; at: number } | null>(null)

  /** Bumped by `newConversation()`: the composer focuses its editor when it changes. */
  composerFocus = $state(0)
  /** Local composer drafts per project, kept until a conversation takes one over. */
  private drafts = new SvelteMap<string, Draft>()
  /** Model and effort chosen for the next conversation of each project. */
  private draftChoices = new SvelteMap<string, { model: string; reasoning: string | null }>()
  /** Conversations being created, per project: concurrent callers share the promise. */
  private starting = new Map<string, Promise<Conversation | null>>()

  private unsubscribe: (() => Promise<void>) | null = null
  private prefsTimer: ReturnType<typeof setTimeout> | null = null

  get currentProject(): Project | null {
    return this.projects.find((p) => p.id === this.currentProjectId) ?? null
  }

  /** The registered project folders (the built-in one left out). */
  get folders(): Project[] {
    return this.projects.filter((p) => !p.chats)
  }

  /** The built-in project of the conversations without a project. */
  get chatsProject(): Project | null {
    return this.projects.find((p) => p.chats) ?? null
  }

  /** The conversations of a project, most recent first (as loaded). */
  sessionsOf(projectId: string): SessionItem[] {
    return this.sessionsByProject.get(projectId) ?? []
  }

  get current(): Conversation | null {
    return this.currentSessionId ? (this.conversations.get(this.currentSessionId) ?? null) : null
  }

  get sessions(): SessionItem[] {
    return this.currentProjectId ? (this.sessionsByProject.get(this.currentProjectId) ?? []) : []
  }

  /** Reloads the model catalog (after a provider/model edit). */
  async reloadModels() {
    try {
      this.models = await ipc.modelsList()
      this.info = await ipc.appInfo()
    } catch (e) {
      this.fail(e)
    }
  }

  get hasProviders(): boolean {
    return this.models.length > 0
  }

  /** Model choice of a `provider/model` selection. */
  model(selection: string | null | undefined): ModelChoice | undefined {
    return this.models.find((m) => m.selection === selection)
  }

  async init() {
    try {
      this.unsubscribe = await ipc.subscribeEvents((e) => this.onEvent(e))
      const [info, prefs, projects] = await Promise.all([
        ipc.appInfo(),
        ipc.prefsGet().catch(() => ({ ...DEFAULT_PREFERENCES })),
        ipc.projectsList(),
      ])
      this.info = info
      this.prefs = prefs
      this.projects = projects
      // Drafts are created here, not while rendering (a state write in a derived is refused).
      for (const p of projects) this.draftFor(p.id)
      this.models = await ipc.modelsList().catch((e) => {
        this.fail(e)
        return []
      })
      // Every project shows its conversations, a project whose folder is missing too.
      await Promise.all(projects.map((p) => this.refreshSessions(p.id)))
      const last =
        projects.find((p) => p.id === prefs.last_project_id && !p.missing) ?? projects.find((p) => p.chats)
      if (last) {
        await this.selectProject(last.id)
        const sid = prefs.last_session_id
        if (sid && this.sessions.some((s) => s.id === sid)) await this.openSession(sid)
      }
    } catch (e) {
      this.startError = ipc.errorMessage(e)
    } finally {
      this.ready = true
    }
  }

  async dispose() {
    await this.unsubscribe?.()
    this.unsubscribe = null
  }

  fail(e: unknown) {
    this.lastError = { message: ipc.errorMessage(e), at: Date.now() }
  }

  // ─── Preferences ──────────────────────────────────────────────────────────

  /** Updates preferences and saves them (debounced). */
  setPrefs(patch: Partial<Preferences>) {
    this.prefs = { ...this.prefs, ...patch }
    if (this.prefsTimer) clearTimeout(this.prefsTimer)
    this.prefsTimer = setTimeout(() => {
      ipc.prefsSet($state.snapshot(this.prefs)).catch((e) => this.fail(e))
    }, 300)
  }

  /**
   * Turns the memory on or off, saved at once (not debounced): it applies from the
   * next run. `null` when saved, else the error.
   */
  async setMemoryEnabled(on: boolean): Promise<string | null> {
    if (this.prefsTimer) clearTimeout(this.prefsTimer)
    const next = { ...this.prefs, memory_enabled: on }
    try {
      await ipc.prefsSet($state.snapshot(next))
      this.prefs = next
      return null
    } catch (e) {
      return ipc.errorMessage(e)
    }
  }

  /** The Computer Use choice, as the backend saved it (its own command saves it). */
  noteComputerUse(enabled: boolean) {
    this.prefs = { ...this.prefs, computer_use_enabled: enabled }
  }

  // ─── Projects ─────────────────────────────────────────────────────────────

  /** Native folder picker, then registers the folder. */
  async addProject(): Promise<Project | null> {
    const picked = await openDialog({ directory: true, multiple: false, title: 'Choisir le dossier du projet' })
    if (!picked || Array.isArray(picked)) return null
    try {
      const project = await ipc.projectAdd(picked)
      this.projects = [project, ...this.projects.filter((p) => p.id !== project.id)]
      this.collapsedProjects.delete(project.id)
      await this.newConversation(project.id)
      return project
    } catch (e) {
      this.fail(e)
      return null
    }
  }

  async removeProject(projectId: string): Promise<boolean> {
    try {
      await ipc.projectRemove(projectId)
      this.projects = this.projects.filter((p) => p.id !== projectId)
      this.sessionsByProject.delete(projectId)
      this.drafts.delete(projectId)
      this.draftChoices.delete(projectId)
      if (this.currentProjectId === projectId) {
        this.currentProjectId = this.chatsProject?.id ?? null
        this.currentSessionId = null
      }
      return true
    } catch (e) {
      this.fail(e)
      return false
    }
  }

  async renameProject(projectId: string, name: string): Promise<boolean> {
    try {
      const p = await ipc.projectRename(projectId, name)
      this.projects = this.projects.map((x) => (x.id === p.id ? p : x))
      return true
    } catch (e) {
      this.fail(e)
      return false
    }
  }

  /**
   * Whether the project's folder is missing. When it is, the folder is asked for
   * (« Dossier de travail introuvable ») and `retry` runs once it is chosen.
   */
  requireFolder(projectId: string, retry: (() => Promise<unknown>) | null): boolean {
    const project = this.projects.find((p) => p.id === projectId)
    if (!project?.missing || project.chats) return false
    this.folderRequest = { projectId, retry }
    return true
  }

  /**
   * « Choisir un dossier… »: the native picker, then the project moves there with
   * its conversations, and the waiting action runs. Cancelling the picker keeps
   * the request open; nothing else changes.
   */
  async chooseFolder(): Promise<boolean> {
    const request = this.folderRequest
    if (!request) return false
    const picked = await openDialog({ directory: true, multiple: false, title: 'Choisir le dossier du projet' })
    if (!picked || Array.isArray(picked)) return false
    try {
      const project = await ipc.projectRelocate(request.projectId, picked)
      this.projects = this.projects.map((p) => (p.id === project.id ? project : p))
      await this.refreshSessions(project.id)
    } catch (e) {
      this.fail(e)
      return false
    }
    if (this.folderRequest === request) this.folderRequest = null
    await request.retry?.()
    return true
  }

  /** Closes the folder request: the selection and the drafts stay as they are. */
  cancelFolderRequest() {
    this.folderRequest = null
  }

  /** Reloads the projects after a folder went missing; true when `projectId`'s folder is gone. */
  private async folderVanished(projectId: string): Promise<boolean> {
    try {
      const projects = await ipc.projectsList()
      this.projects = projects
      return projects.some((p) => p.id === projectId && p.missing)
    } catch {
      return false
    }
  }

  async selectProject(projectId: string) {
    this.currentProjectId = projectId
    this.draftFor(projectId)
    if (this.current && this.current.projectId !== projectId) this.currentSessionId = null
    this.setPrefs({ last_project_id: projectId })
    await this.refreshSessions(projectId)
  }

  async refreshSessions(projectId = this.currentProjectId) {
    if (!projectId) return
    try {
      this.sessionsByProject.set(projectId, await ipc.sessionsList(projectId))
    } catch (e) {
      this.fail(e)
    }
  }

  // ─── Conversations ────────────────────────────────────────────────────────

  /**
   * « Nouvelle conversation »: shows the composer of a project (the current
   * one by default) with its draft. No session is opened here: the first
   * valid send does it through `startConversation`. Repeated calls keep the
   * same draft.
   */
  async newConversation(projectId = this.currentProjectId): Promise<void> {
    if (!projectId) return
    if (this.requireFolder(projectId, () => this.newConversation(projectId))) return
    if (projectId !== this.currentProjectId) {
      this.currentProjectId = projectId
      this.setPrefs({ last_project_id: projectId })
      void this.refreshSessions(projectId)
    }
    const leaving = this.current
    // The next conversation's model defaults to the one being left (if configured).
    if (leaving && leaving.projectId === projectId) {
      this.draftChoices.set(projectId, { model: leaving.model, reasoning: leaving.reasoning })
    }
    this.draftFor(projectId)
    this.currentSessionId = null
    this.setPrefs({ last_session_id: null })
    this.composerFocus += 1
  }

  /** The local composer draft of a project (created on first use, then kept). */
  draftFor(projectId: string): Draft {
    const existing = this.drafts.get(projectId)
    if (existing) return existing
    const fresh = $state<Draft>({ text: '', attachments: [], skills: [] })
    this.drafts.set(projectId, fresh)
    return fresh
  }

  /** Empties the draft of a project: called by the composer once its first submit succeeded. */
  clearProjectDraft(projectId: string) {
    const draft = this.draftFor(projectId)
    draft.text = ''
    draft.attachments = []
    draft.skills = []
  }

  /** Model of the next conversation of a project: its own choice if still configured, else the first configured. */
  draftModel(projectId: string): string | null {
    const chosen = this.draftChoices.get(projectId)?.model
    if (chosen && this.model(chosen)) return chosen
    return this.models[0]?.selection ?? null
  }

  /** Effort of the next conversation of a project; kept only while its model is the draft model. */
  draftReasoning(projectId: string): string | null {
    const choice = this.draftChoices.get(projectId)
    return choice && choice.model === this.draftModel(projectId) ? choice.reasoning : null
  }

  setDraftModel(projectId: string, model: string, reasoning: string | null = null) {
    this.draftChoices.set(projectId, { model, reasoning })
  }

  /**
   * Opens the session of a new conversation in the current project, which
   * takes over the project draft. This is the only path that creates a session
   * from the composer. Concurrent callers for one project share the same
   * creation; a conversation already shown for the project is returned as is.
   * The project draft is kept until `clearProjectDraft`. On failure the draft
   * is kept and the error is reported in `lastError`.
   */
  startConversation(model?: string, reasoning?: string): Promise<Conversation | null> {
    const projectId = this.currentProjectId
    if (!projectId) {
      this.fail('Choisissez un projet pour démarrer une conversation.')
      return Promise.resolve(null)
    }
    // The draft is kept; once the folder is found, the composer gets the focus back to send it.
    if (this.requireFolder(projectId, async () => void (this.composerFocus += 1))) return Promise.resolve(null)
    const pending = this.starting.get(projectId)
    if (pending) return pending
    const shown = this.current
    if (shown && shown.projectId === projectId) return Promise.resolve(shown)
    const creation = this.createConversation(projectId, model, reasoning).finally(() => {
      this.starting.delete(projectId)
    })
    this.starting.set(projectId, creation)
    return creation
  }

  private createConversation(projectId: string, model?: string, reasoning?: string): Promise<Conversation | null> {
    const selection = model ?? this.draftModel(projectId) ?? undefined
    const effort = model === undefined ? (reasoning ?? this.draftReasoning(projectId) ?? undefined) : reasoning
    return this.open(projectId, null, selection, effort, { draftFrom: this.draftFor(projectId), reveal: 'draft' })
  }

  /**
   * Reopens a stored session by id in its own registered project (`/resume
   * <id>`), through the desktop registry. Returns the refusal message.
   */
  async resumeSession(sessionId: string): Promise<string | null> {
    try {
      const item = await ipc.sessionLocate(sessionId)
      if (!item.project_id)
        return `La session ${sessionId} appartient à un dossier qui n’est pas un projet Bricks (${item.working_dir ?? 'dossier inconnu'}). Ajoutez ce dossier avec « Nouveau projet ».`
      if (this.currentProjectId !== item.project_id) await this.selectProject(item.project_id)
      if (this.requireFolder(item.project_id, () => this.open(item.project_id!, sessionId))) return null
      const conv = await this.open(item.project_id, sessionId)
      return conv ? null : (this.lastError?.message ?? 'Reprise impossible.')
    } catch (e) {
      return ipc.errorMessage(e)
    }
  }

  /** Opens (or shows the already open) stored conversation. */
  async openSession(sessionId: string): Promise<Conversation | null> {
    const existing = this.conversations.get(sessionId)
    if (existing) {
      this.show(existing)
      return existing
    }
    const projectId = this.projectOf(sessionId) ?? this.currentProjectId
    if (!projectId) return null
    if (this.requireFolder(projectId, () => this.openSession(sessionId))) return null
    return this.open(projectId, sessionId)
  }

  private async open(
    projectId: string,
    sessionId: string | null,
    model?: string,
    reasoning?: string,
    options: OpenOptions = {},
  ): Promise<Conversation | null> {
    try {
      const opened = await ipc.sessionOpen({
        project_id: projectId,
        session_id: sessionId,
        model: model ?? null,
        reasoning: reasoning ?? null,
      })
      let conv = this.conversations.get(opened.session.id)
      if (!conv) {
        conv = new Conversation(opened, projectId)
        const draftFrom = options.draftFrom
        if (draftFrom) {
          conv.draft.text = draftFrom.text
          conv.draft.attachments = $state.snapshot(draftFrom.attachments)
          conv.draft.skills = $state.snapshot(draftFrom.skills)
        }
        this.conversations.set(conv.sessionId, conv)
        const c = conv
        Promise.all([ipc.sessionAgents(c.sessionId), ipc.sessionChangesets(c.sessionId)])
          .then(([records, sets]) => c.hydrateAgents(records, sets))
          .catch(() => {})
      }
      this.upsertSession(projectId, opened.session)
      if (this.revealed(projectId, options.reveal ?? 'always')) this.show(conv)
      return conv
    } catch (e) {
      // The folder went missing since the projects were loaded: ask for it, then open again.
      if (isFolderMissing(e) && (await this.folderVanished(projectId))) {
        this.requireFolder(projectId, () => this.open(projectId, sessionId, model, reasoning, options))
        return null
      }
      this.fail(e)
      return null
    }
  }

  /** Whether a conversation of `projectId` that has just opened takes the view now. */
  private revealed(projectId: string, reveal: Reveal): boolean {
    if (reveal === 'always') return true
    if (reveal === 'never') return false
    // Still on the draft of this project: the user did not navigate away meanwhile.
    return this.currentProjectId === projectId && this.currentSessionId === null
  }

  private show(conv: Conversation) {
    if (conv.projectId) {
      this.currentProjectId = conv.projectId
      this.collapsedProjects.delete(conv.projectId)
    }
    this.currentSessionId = conv.sessionId
    this.setPrefs({ last_project_id: conv.projectId, last_session_id: conv.sessionId })
  }

  async renameSession(sessionId: string, title: string) {
    try {
      const item = await ipc.sessionRename(sessionId, title)
      this.onSessionUpdated(item)
    } catch (e) {
      this.fail(e)
    }
  }

  /** Deletes after the caller's confirmation; closes the session first. */
  async deleteSession(sessionId: string): Promise<boolean> {
    try {
      await ipc.sessionDelete(sessionId)
      this.forget(sessionId)
      return true
    } catch (e) {
      this.fail(e)
      return false
    }
  }

  async closeSession(sessionId: string, cancel = false) {
    try {
      await ipc.sessionClose(sessionId, cancel)
      this.conversations.delete(sessionId)
      if (this.currentSessionId === sessionId) this.currentSessionId = null
    } catch (e) {
      this.fail(e)
    }
  }

  search(query: string, allProjects = false): Promise<SessionSearchHit[]> {
    return ipc.sessionsSearch(allProjects ? null : this.currentProjectId, query)
  }

  /** The question timer of the preferences, as the engine takes it. */
  askTimer(): AskTimer {
    const t = this.prefs.ask_timer
    if (!t.enabled) return { mode: 'off' }
    return t.mode === 'custom' ? { mode: 'custom', seconds: t.seconds } : { mode: 'smart' }
  }

  /** Answers a question of `Ask` (a choice, a free text, or a skip). Returns the refusal. */
  answerQuestion(conv: Conversation, questionId: string, answer: QuestionAnswer): Promise<string | null> {
    return this.command(conv.sessionId, { type: 'answer_question', question_id: questionId, answer })
  }

  /**
   * « Enregistrer et relancer »: replaces a sent message with `text` (its
   * attachments kept) and runs the conversation again from it. Returns the
   * refusal message, or null once the engine accepted it.
   */
  async reviseMessage(conv: Conversation, item: UserItem, text: string): Promise<string | null> {
    if (conv.running) return 'Une exécution est en cours : attendez sa fin ou annulez-la.'
    let index: number | null
    let blocks: PromptBlock[]
    try {
      index = await conv.userIndexOf(item)
      blocks = await conv.revisedBlocks(item, text)
    } catch (e) {
      return ipc.errorMessage(e)
    }
    if (index === null) return 'Ce message ne peut pas être modifié : rouvrez la conversation.'
    if (blocks.length === 0) return 'Le message est vide.'
    conv.revisingItemId = item.id
    const error = await this.command(conv.sessionId, {
      type: 'revise',
      user_index: index,
      original: item.text,
      prompt: { blocks },
    })
    if (error) conv.revisingItemId = null
    return error
  }

  /**
   * Sends an engine command to a session. `display` is the command as typed
   * (e.g. `/skill debug`) for the user message of a submit; it never reaches
   * the engine. Errors are returned, not thrown.
   */
  async command(sessionId: string, command: Command, display?: string | null): Promise<string | null> {
    // The label of a submit is pending from the call on: the run it starts can
    // announce itself before the call returns. A refusal takes it back.
    const submitted = command.type === 'submit' ? this.conversations.get(sessionId) : undefined
    submitted?.setPendingSubmitLabel(display || null)
    // Every message carries the question timer as set now: the engine keeps it
    // for the whole run the message starts.
    const sent: Command =
      command.type === 'submit' || command.type === 'revise'
        ? { ...command, prompt: { ...command.prompt, ask_timer: this.askTimer() } }
        : command
    try {
      await ipc.sessionCommand(sessionId, sent, display ?? null)
      return null
    } catch (e) {
      submitted?.setPendingSubmitLabel(null)
      return ipc.errorMessage(e)
    }
  }

  /**
   * Sets a session's permission policy. `asDefault` also makes it the policy
   * new conversations start from. Errors are returned, not thrown.
   */
  async setPermissions(sessionId: string, policy: SessionPolicy, asDefault = false): Promise<string | null> {
    const chosen = { ...policy, chosen: true }
    const refused = await this.command(sessionId, { type: 'set_permission_policy', policy: chosen })
    if (refused || !asDefault) return refused
    try {
      await ipc.permissionsDefaultSet(chosen)
      return null
    } catch (e) {
      return ipc.errorMessage(e)
    }
  }

  // ─── Events ───────────────────────────────────────────────────────────────

  private onEvent(e: DesktopEvent) {
    switch (e.kind) {
      case 'engine': {
        const env = e.envelope as Envelope
        this.conversations.get(env.session_id)?.apply(env)
        if (env.type === 'run_finished' || env.type === 'run_started') {
          const pid = e.project_id ?? this.projectOf(env.session_id)
          if (pid) this.refreshSessions(pid)
        }
        break
      }
      case 'session_state': {
        for (const [pid, list] of this.sessionsByProject) {
          const i = list.findIndex((s) => s.id === e.session_id)
          if (i >= 0) {
            const next = [...list]
            next[i] = { ...list[i], open: e.open, running: e.running }
            this.sessionsByProject.set(pid, next)
          }
        }
        if (!e.open) this.conversations.delete(e.session_id)
        break
      }
      case 'session_updated':
        this.onSessionUpdated(e.session)
        break
      case 'session_deleted':
        this.forget(e.session_id)
        break
      case 'lagged': {
        const conv = this.conversations.get(e.session_id)
        if (conv?.projectId) {
          // The reopened conversation keeps its unsent draft, and takes the view only if it had it.
          const wasShown = this.currentSessionId === e.session_id
          this.conversations.delete(e.session_id)
          this.open(conv.projectId, e.session_id, undefined, undefined, {
            draftFrom: conv.draft,
            reveal: wasShown ? 'always' : 'never',
          })
        }
        break
      }
      case 'run_labeled':
        this.conversations.get(e.session_id)?.applyRunLabel(e.label)
        break
      case 'change_recorded':
        changeListeners.forEach((l) => l(e.record))
        break
    }
  }

  private onSessionUpdated(item: SessionItem) {
    this.conversations.get(item.id)?.updateSession(item)
    const pid = item.project_id ?? this.projectOf(item.id)
    if (pid) this.upsertSession(pid, item)
  }

  private upsertSession(projectId: string, item: SessionItem) {
    const list = this.sessionsByProject.get(projectId) ?? []
    const rest = list.filter((s) => s.id !== item.id)
    this.sessionsByProject.set(
      projectId,
      [item, ...rest].sort((a, b) => b.updated_at - a.updated_at),
    )
  }

  private forget(sessionId: string) {
    this.conversations.delete(sessionId)
    for (const [pid, list] of this.sessionsByProject)
      this.sessionsByProject.set(
        pid,
        list.filter((s) => s.id !== sessionId),
      )
    if (this.currentSessionId === sessionId) this.currentSessionId = null
  }

  private projectOf(sessionId: string): string | null {
    for (const [pid, list] of this.sessionsByProject)
      if (list.some((s) => s.id === sessionId)) return pid
    return this.conversations.get(sessionId)?.projectId ?? null
  }
}

type ChangeListener = (record: import('$lib/ipc/client').ChangeRecord) => void
const changeListeners = new Set<ChangeListener>()

/** Called for every new change-journal record; returns an unsubscribe. */
export function onChangeRecorded(listener: ChangeListener): () => void {
  changeListeners.add(listener)
  return () => changeListeners.delete(listener)
}

export const app = new AppStore()

// The reader reads the edit operations of each open conversation from its timeline.
reader.connect((sessionId) => app.conversations.get(sessionId)?.items ?? [])
