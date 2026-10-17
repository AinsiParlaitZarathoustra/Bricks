// Application state: configuration, projects, conversations, the open
// sessions and the single event subscription that feeds them.
// Owner: orchestrator. Components read `app` and call its methods; they
// never call the provider or recompute engine state themselves.

import { open as openDialog } from '@tauri-apps/plugin-dialog'
import { SvelteMap } from 'svelte/reactivity'
import * as ipc from '$lib/ipc/client'
import type {
  AppInfo,
  DesktopEvent,
  Preferences,
  Project,
  SessionItem,
  SessionSearchHit,
} from '$lib/ipc/client'
import type { Command, Envelope, ModelChoice } from '$lib/ipc/engine'
import { Conversation } from './conversation.svelte'

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
  tool_approval: 'standard',
  colors: { light: { accent: null, background: null, panel: null }, dark: { accent: null, background: null, panel: null } },
  terminal_ratio: null,
}

export type RightView = 'context' | 'changes'

class AppStore {
  ready = $state(false)
  /** Fatal start-up error (the bridge itself failed). */
  startError = $state<string | null>(null)

  info = $state<AppInfo | null>(null)
  models = $state<ModelChoice[]>([])
  prefs = $state<Preferences>({ ...DEFAULT_PREFERENCES })

  projects = $state<Project[]>([])
  sessionsByProject = new SvelteMap<string, SessionItem[]>()
  conversations = new SvelteMap<string, Conversation>()

  currentProjectId = $state<string | null>(null)
  currentSessionId = $state<string | null>(null)
  /** Right panel view requested from the top bar. */
  rightView = $state<RightView>('context')

  /** Last user-facing error of an action (shown as a toast by the UI). */
  lastError = $state<{ message: string; at: number } | null>(null)

  private unsubscribe: (() => Promise<void>) | null = null
  private prefsTimer: ReturnType<typeof setTimeout> | null = null

  get currentProject(): Project | null {
    return this.projects.find((p) => p.id === this.currentProjectId) ?? null
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
      this.models = await ipc.modelsList().catch((e) => {
        this.fail(e)
        return []
      })
      const last = projects.find((p) => p.id === prefs.last_project_id && !p.missing)
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

  /**
   * Updates preferences and saves them (debounced). The tool approval
   * policy changes engine decisions: it is saved at once and rolled back
   * if the save fails, so the shown choice is always the applied one.
   */
  setPrefs(patch: Partial<Preferences>) {
    if (patch.tool_approval !== undefined && patch.tool_approval !== this.prefs.tool_approval) {
      const previous = this.prefs.tool_approval
      this.prefs = { ...this.prefs, ...patch }
      if (this.prefsTimer) clearTimeout(this.prefsTimer)
      this.prefsTimer = null
      ipc.prefsSet($state.snapshot(this.prefs)).catch((e) => {
        this.prefs = { ...this.prefs, tool_approval: previous }
        this.fail(e)
      })
      return
    }
    this.prefs = { ...this.prefs, ...patch }
    if (this.prefsTimer) clearTimeout(this.prefsTimer)
    this.prefsTimer = setTimeout(() => {
      ipc.prefsSet($state.snapshot(this.prefs)).catch((e) => this.fail(e))
    }, 300)
  }

  // ─── Projects ─────────────────────────────────────────────────────────────

  /** Native folder picker, then registers the folder. */
  async addProject(): Promise<Project | null> {
    const picked = await openDialog({ directory: true, multiple: false, title: 'Choisir le dossier du projet' })
    if (!picked || Array.isArray(picked)) return null
    try {
      const project = await ipc.projectAdd(picked)
      this.projects = [project, ...this.projects.filter((p) => p.id !== project.id)]
      await this.selectProject(project.id)
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
      if (this.currentProjectId === projectId) {
        this.currentProjectId = null
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

  async selectProject(projectId: string) {
    this.currentProjectId = projectId
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

  /** Starts a new conversation in the current project. */
  async newConversation(model?: string, reasoning?: string): Promise<Conversation | null> {
    const projectId = this.currentProjectId
    if (!projectId) return null
    // The engine needs a model: without an explicit choice, keep the current
    // conversation's (if still configured), else the first configured one.
    if (!model) {
      const current = this.current?.model
      model = (current && this.model(current) ? current : this.models[0]?.selection) ?? undefined
      if (reasoning === undefined && model === current) reasoning = this.current?.reasoning ?? undefined
    }
    return this.open(projectId, null, model, reasoning)
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
    return this.open(projectId, sessionId)
  }

  private async open(
    projectId: string,
    sessionId: string | null,
    model?: string,
    reasoning?: string,
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
        this.conversations.set(conv.sessionId, conv)
        const c = conv
        Promise.all([ipc.sessionAgents(c.sessionId), ipc.sessionChangesets(c.sessionId)])
          .then(([records, sets]) => c.hydrateAgents(records, sets))
          .catch(() => {})
      }
      this.upsertSession(projectId, opened.session)
      this.show(conv)
      return conv
    } catch (e) {
      this.fail(e)
      return null
    }
  }

  private show(conv: Conversation) {
    if (conv.projectId) this.currentProjectId = conv.projectId
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

  /** Sends an engine command to a session. Errors are returned, not thrown. */
  async command(sessionId: string, command: Command): Promise<string | null> {
    try {
      await ipc.sessionCommand(sessionId, command)
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
          this.conversations.delete(e.session_id)
          this.open(conv.projectId, e.session_id)
        }
        break
      }
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
