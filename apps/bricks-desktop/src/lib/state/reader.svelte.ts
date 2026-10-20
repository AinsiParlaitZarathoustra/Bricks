// The reader of the right column (Sprint 14.6.2): the files a conversation
// names, read by Rust in windows of 400 lines, and the edit operations recorded
// on each of them. Tabs are kept per session.
//
// A tab is one file of one workspace. Its identity is (session, agent, file):
// the agent is '' for the main agent, the file is its full path, canonical once
// Rust has read it. The name is never an identity: two files may share a
// basename. Opening an identity already open activates its tab.
//
// Operations come from the `edit` items of the session's conversation, in
// order. Each recorded patch stands on its own (never merged, never rebuilt): a
// patch that was not recorded, or was truncated, is unavailable.
//
// Owner: logique 2. Components read `reader` and call its methods.

import { SvelteMap } from 'svelte/reactivity'
import { readerFileRead, type ReaderFileRequest, type ReaderFileWindow } from '$lib/ipc/reader'
import type { TimelineItem } from './conversation.svelte'

/** Lines in one window: the window of `project_file_read`. */
export const WINDOW_LINES = 400

export type ReaderMode = 'source' | 'preview' | 'changes'

/** What a caller asks the reader to open. */
export interface ReaderTarget {
  sessionId: string
  projectId: string | null
  /** The agent whose workspace holds the file; absent or `null`: the main agent. */
  agentId?: string | null
  /** The path as the conversation gives it: relative to the workspace, or absolute. */
  path: string
  /** The absolute path the conversation recorded for the file, when it has one. */
  absolute?: string | null
  /** 1-based line to show (the window is centred on it), or the first line of `endLine`. */
  line?: number | null
  endLine?: number | null
}

/** A window of a file, as the reader shows it. */
export interface ReaderWindow {
  /** The number of the first line of `lines` (1-based). */
  start: number
  lines: string[]
  /** Lines of the file that were read: a lower bound when the read limit cut it. */
  totalLines: number
  truncated: boolean
  loading: boolean
  error: string | null
}

/** One applied edit of a file, as the conversation recorded it. */
export interface PatchOperation {
  /** Stable key: the edit item and the position of the file in it. */
  key: string
  /** The agent that made the edit; `null` for the main agent. */
  agentId: string | null
  toolCallId: string
  kind: string
  added: number
  removed: number
  /** The unified diff of this edit, or `null` when it was not recorded or was truncated. */
  patch: string | null
  /** The recorded diff was cut at the engine's limit: `patch` is then `null`. */
  truncated: boolean
}

/**
 * The fields of an edited file the reader reads (`EditItem.files[i]`). The
 * absolute path and the patch are recorded by the engine; they are optional
 * for sessions that predate them.
 */
interface EditedFile {
  path: string
  kind: string
  added: number
  removed: number
  absolute?: string | null
  patch?: string | null
  patchTruncated?: boolean
}

/** The timeline of a session, as the reader reads it. */
export type ItemsOf = (sessionId: string) => readonly TimelineItem[]

/** Normalises a path for comparison: no empty or `.` segment, no trailing slash. `..` is kept (Rust refuses it). */
export function normalizePath(path: string): string {
  const parts = path.split('/').filter((part) => part !== '' && part !== '.')
  const joined = parts.join('/')
  return path.startsWith('/') ? `/${joined}` : joined
}

function baseName(path: string): string {
  return path.slice(path.lastIndexOf('/') + 1)
}

function messageOf(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

const MARKDOWN = /\.(md|markdown|mdx)$/i

/** The patch an edited file recorded, or `null` when it is missing or was truncated. */
function recordedPatch(file: EditedFile): string | null {
  if (file.patchTruncated === true) return null
  return typeof file.patch === 'string' ? file.patch : null
}

/** Whether an edited file is the file of `tab`. */
function isTabFile(tab: ReaderTab, agentId: string | null, file: EditedFile): boolean {
  if (typeof file.absolute === 'string' && file.absolute !== '') {
    return tab.hasKey(normalizePath(file.absolute))
  }
  // A relative path is relative to the workspace of its agent: only that agent's tabs match it.
  return tab.isRelative && (agentId ?? '') === tab.agentId && normalizePath(file.path) === tab.path
}

/** One open file of a session. Read it through `reader`; its fields are the reader's state. */
export class ReaderTab {
  /** Fixed at creation: session, agent and the path first asked. */
  readonly id: string
  readonly sessionId: string
  /** The agent whose workspace holds the file; `''` for the main agent. */
  readonly agentId: string
  /** The file's name as the conversation gives it. */
  readonly title: string
  /** The path as the conversation gives it, normalised: relative to the workspace, or absolute. */
  readonly path: string
  /** The path sent to Rust for every window of this tab. */
  readonly request: string
  readonly isMarkdown: boolean
  readonly isSvg: boolean

  /** The canonical absolute path once Rust has read the file; until then the path asked. */
  fullPath = $state('')
  /** Language of highlight.js, from Rust, when known. */
  language = $state<string | null>(null)
  window = $state<ReaderWindow | null>(null)
  scrollTop = $state(0)
  /** The mode the user picked; `mode` falls back to `source` when it is not available. */
  requested = $state<ReaderMode>('source')
  /** Paths that identify this file: the path asked, then the canonical path. */
  private keys = $state<string[]>([])
  /** Token of the latest window request: the answers of older requests are ignored. */
  private loadToken = 0
  private closed = false
  private readonly opsOf: (tab: ReaderTab) => PatchOperation[]
  private readonly opsCache = $derived.by(() => this.opsOf(this))

  constructor(
    init: { sessionId: string; agentId: string; path: string; request: string },
    opsOf: (tab: ReaderTab) => PatchOperation[],
  ) {
    this.sessionId = init.sessionId
    this.agentId = init.agentId
    this.path = init.path
    this.request = init.request
    this.title = baseName(init.path) || baseName(init.request)
    this.id = `${init.sessionId}|${init.agentId}|${init.request}`
    this.isMarkdown = MARKDOWN.test(init.path)
    this.isSvg = /\.svg$/i.test(init.path)
    this.opsOf = opsOf
    this.fullPath = init.request
    this.keys = [init.request]
  }

  /** Whether the path was given relative to the workspace. */
  get isRelative(): boolean {
    return !this.path.startsWith('/')
  }

  /** The recorded edit operations of this file, in conversation order. */
  get operations(): PatchOperation[] {
    return this.opsCache
  }

  /** The modes this file offers: source always, preview for Markdown, changes when edited. */
  get modes(): ReaderMode[] {
    const modes: ReaderMode[] = ['source']
    if (this.isMarkdown || this.isSvg) modes.push('preview')
    if (this.operations.length > 0) modes.push('changes')
    return modes
  }

  /** The mode shown: the requested one when this file offers it, else `source`. */
  get mode(): ReaderMode {
    return this.modes.includes(this.requested) ? this.requested : 'source'
  }

  hasKey(key: string): boolean {
    return this.keys.includes(key)
  }

  /** @internal Called by `ReaderStore`. */
  addKey(key: string): void {
    if (!this.keys.includes(key)) this.keys.push(key)
  }

  /** @internal */
  accepts(token: number): boolean {
    return !this.closed && this.loadToken === token
  }

  /** @internal */
  close(): void {
    this.closed = true
  }

  /** @internal */
  beginLoad(token: number): void {
    this.loadToken = token
    const previous = this.window
    this.window = {
      start: previous?.start ?? 1,
      lines: [],
      totalLines: previous?.totalLines ?? 0,
      truncated: previous?.truncated ?? false,
      loading: true,
      error: null,
    }
  }

  /** @internal */
  receive(read: ReaderFileWindow): void {
    this.language = read.language ?? null
    this.window = {
      start: read.start,
      lines: read.lines,
      totalLines: read.totalLines,
      truncated: read.truncated,
      loading: false,
      error: null,
    }
    this.fullPath = read.absolute
    this.addKey(read.absolute)
  }

  /** @internal */
  fail(message: string): void {
    const previous = this.window
    this.window = {
      start: previous?.start ?? 1,
      lines: [],
      totalLines: previous?.totalLines ?? 0,
      truncated: previous?.truncated ?? false,
      loading: false,
      error: message,
    }
  }
}

/** Tabs of the reader, per session. Use the `reader` instance; the class is exported for tests. */
export class ReaderStore {
  /** Bumped by every `open`: the right column shows the reader when it changes. */
  openRequests = $state(0)

  private tabsBySession = new SvelteMap<string, ReaderTab[]>()
  private activeBySession = new SvelteMap<string, string>()
  private itemsOf: ItemsOf = () => []
  private readonly read: (request: ReaderFileRequest) => Promise<ReaderFileWindow>
  private seq = 0

  constructor(read: (request: ReaderFileRequest) => Promise<ReaderFileWindow> = readerFileRead) {
    this.read = read
  }

  /** Gives the reader the timeline of each session, from which the edit operations are read. */
  connect(itemsOf: ItemsOf): void {
    this.itemsOf = itemsOf
  }

  /** The tabs of a session, in the order they were opened. */
  tabs(sessionId: string): readonly ReaderTab[] {
    return this.tabsBySession.get(sessionId) ?? []
  }

  /** The tab shown in a session, or `null` when none is open. */
  active(sessionId: string): ReaderTab | null {
    const id = this.activeBySession.get(sessionId)
    return this.tabs(sessionId).find((tab) => tab.id === id) ?? null
  }

  /**
   * Opens a file: its tab is activated when the same file of the same agent is
   * already open (no second tab), else a tab is created and read. `line` and
   * `endLine` centre the window on the line, or show the range. Returns the tab.
   */
  open(target: ReaderTarget): ReaderTab {
    this.openRequests += 1
    const agentId = target.agentId ?? ''
    const request = normalizePath(target.absolute || target.path)
    const range = { line: target.line ?? null, endLine: target.endLine ?? null }

    const existing = this.tabs(target.sessionId).find(
      (tab) => tab.agentId === agentId && tab.hasKey(request),
    )
    if (existing) {
      this.show(existing)
      if (range.line !== null || existing.window === null || existing.window.error !== null) {
        this.fetch(existing, range)
      }
      return existing
    }

    const path = normalizePath(target.path || request)
    const tab = new ReaderTab(
      { sessionId: target.sessionId, agentId, path, request },
      (owner) => this.operationsOf(owner),
    )
    this.tabsBySession.set(target.sessionId, [...this.tabs(target.sessionId), tab])
    this.show(tab)
    this.fetch(tab, range)
    return tab
  }

  /** Shows the tab `tabId` of its session. */
  activate(tabId: string): void {
    const tab = this.find(tabId)
    if (tab) this.show(tab)
  }

  /** Closes the tab `tabId`. Closing the shown tab shows the next one, else the previous one. */
  close(tabId: string): void {
    const tab = this.find(tabId)
    if (tab) this.remove(tab)
  }

  /** Shows `mode` for the tab `tabId`; `false` when that tab does not offer it. */
  setMode(tabId: string, mode: ReaderMode): boolean {
    const tab = this.find(tabId)
    if (!tab || !tab.modes.includes(mode)) return false
    tab.requested = mode
    return true
  }

  /** Keeps the scroll position of a tab, to restore it when the tab is shown again. */
  rememberScroll(tabId: string, px: number): void {
    const tab = this.find(tabId)
    if (tab) tab.scrollTop = Math.max(0, px)
  }

  /** Reads the window of 400 lines that starts at `start` (1-based). Near the end of a file the window may start earlier, so that it is full. */
  loadWindow(tabId: string, start: number): void {
    const tab = this.find(tabId)
    if (!tab) return
    const first = Number.isFinite(start) ? Math.max(1, Math.floor(start)) : 1
    this.fetch(tab, { line: first, endLine: first + WINDOW_LINES - 1 })
  }

  /** Drops every tab of a session (its conversation is gone). */
  forget(sessionId: string): void {
    for (const tab of this.tabs(sessionId)) tab.close()
    this.tabsBySession.delete(sessionId)
    this.activeBySession.delete(sessionId)
  }

  private find(tabId: string): ReaderTab | null {
    for (const tabs of this.tabsBySession.values()) {
      const tab = tabs.find((t) => t.id === tabId)
      if (tab) return tab
    }
    return null
  }

  private show(tab: ReaderTab): void {
    this.activeBySession.set(tab.sessionId, tab.id)
  }

  private remove(tab: ReaderTab): void {
    const list = this.tabs(tab.sessionId)
    const index = list.indexOf(tab)
    if (index < 0) return
    tab.close()
    const rest = list.filter((other) => other !== tab)
    this.tabsBySession.set(tab.sessionId, rest)
    if (this.activeBySession.get(tab.sessionId) === tab.id) {
      const next = rest[index] ?? rest[index - 1]
      if (next) this.activeBySession.set(tab.sessionId, next.id)
      else this.activeBySession.delete(tab.sessionId)
    }
  }

  private fetch(tab: ReaderTab, range: { line: number | null; endLine: number | null }): void {
    const token = ++this.seq
    tab.beginLoad(token)
    this.read({
      sessionId: tab.sessionId,
      agentId: tab.agentId === '' ? null : tab.agentId,
      path: tab.request,
      line: range.line,
      endLine: range.endLine,
    })
      .then((read) => {
        if (!tab.accepts(token)) return
        tab.receive(read)
        this.adopt(tab)
      })
      .catch((e: unknown) => {
        if (tab.accepts(token)) tab.fail(messageOf(e))
      })
  }

  /**
   * Once the canonical path of `tab` is known, a tab of the same file opened
   * under another spelling (relative, or another absolute path) is the same
   * tab: this one goes away and the other one takes the view.
   */
  private adopt(tab: ReaderTab): void {
    const canonical = tab.fullPath
    const twin = this.tabs(tab.sessionId).find(
      (other) => other !== tab && other.agentId === tab.agentId && other.hasKey(canonical),
    )
    if (!twin) return
    if (this.activeBySession.get(tab.sessionId) === tab.id) this.show(twin)
    this.remove(tab)
  }

  private operationsOf(tab: ReaderTab): PatchOperation[] {
    const found: PatchOperation[] = []
    for (const item of this.itemsOf(tab.sessionId)) {
      if (item.kind !== 'edit') continue
      const files = item.files as readonly EditedFile[]
      files.forEach((file, index) => {
        if (!isTabFile(tab, item.agentId ?? null, file)) return
        found.push({
          key: `${item.id}#${index}`,
          agentId: item.agentId ?? null,
          toolCallId: item.toolCallId,
          kind: file.kind,
          added: file.added,
          removed: file.removed,
          patch: recordedPatch(file),
          truncated: file.patchTruncated === true,
        })
      })
    }
    return found
  }
}

export const reader = new ReaderStore()
