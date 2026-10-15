// Typed client for the Bricks Desktop IPC commands (see
// src-tauri/src/lib.rs and contract.rs). Owner: orchestrator.
// Tauri maps the Rust `snake_case` arguments to `camelCase` keys.

import { Channel, invoke } from '@tauri-apps/api/core'
import type { AppInfo } from './bindings/AppInfo'
import type { ChangeJournalInfo } from './bindings/ChangeJournalInfo'
import type { ChangeRecord } from './bindings/ChangeRecord'
import type { DesktopEvent } from './bindings/DesktopEvent'
import type { FileDiff } from './bindings/FileDiff'
import type { GitArea } from './bindings/GitArea'
import type { GitCommit } from './bindings/GitCommit'
import type { GitStatus } from './bindings/GitStatus'
import type { IpcErrorCode } from './bindings/IpcErrorCode'
import type { OpenedSession } from './bindings/OpenedSession'
import type { OpenSessionRequest } from './bindings/OpenSessionRequest'
import type { Preferences } from './bindings/Preferences'
import type { Project } from './bindings/Project'
import type { PtyEvent } from './bindings/PtyEvent'
import type { PtyInfo } from './bindings/PtyInfo'
import type { PtyOpenRequest } from './bindings/PtyOpenRequest'
import type { SavedAttachment } from './bindings/SavedAttachment'
import type { SessionItem } from './bindings/SessionItem'
import type { SessionSearchHit } from './bindings/SessionSearchHit'
import type { Command, ModelChoice, Snapshot } from './engine'

export type * from './bindings/AppInfo'
export type * from './bindings/ChangeJournalInfo'
export type * from './bindings/ChangeRecord'
export type * from './bindings/ChangeFile'
export type * from './bindings/DesktopEvent'
export type * from './bindings/FileDiff'
export type * from './bindings/GitArea'
export type * from './bindings/GitCommit'
export type * from './bindings/GitEntry'
export type * from './bindings/GitStatus'
export type * from './bindings/HistoryBlock'
export type * from './bindings/HistoryMessage'
export type * from './bindings/HistoryRole'
export type * from './bindings/IpcErrorCode'
export type * from './bindings/OpenedSession'
export type * from './bindings/OpenSessionRequest'
export type * from './bindings/Preferences'
export type * from './bindings/Project'
export type * from './bindings/ProviderInfo'
export type * from './bindings/PtyEvent'
export type * from './bindings/PtyInfo'
export type * from './bindings/PtyOpenRequest'
export type * from './bindings/SavedAttachment'
export type * from './bindings/SearchMatch'
export type * from './bindings/SessionItem'
export type * from './bindings/SessionSearchHit'
export type * from './bindings/Theme'

/** A failed command, as the Rust side reports it. */
export class IpcError extends Error {
  constructor(
    readonly code: IpcErrorCode | 'transport',
    message: string,
  ) {
    super(message)
    this.name = 'IpcError'
  }
}

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args)
  } catch (e) {
    if (e && typeof e === 'object' && 'code' in e && 'message' in e) {
      const err = e as { code: IpcErrorCode; message: string }
      throw new IpcError(err.code, err.message)
    }
    throw new IpcError('transport', String(e))
  }
}

/** Message to show for any thrown value. */
export function errorMessage(e: unknown): string {
  if (e instanceof Error) return e.message
  return String(e)
}

// ─── Application ─────────────────────────────────────────────────────────────

export const appInfo = () => call<AppInfo>('app_info')
export const modelsList = () => call<ModelChoice[]>('models_list')
export const prefsGet = () => call<Preferences>('prefs_get')
export const prefsSet = (prefs: Preferences) => call<void>('prefs_set', { prefs })

/** Subscribes to every session's events. Returns an unsubscribe function. */
export async function subscribeEvents(
  handler: (event: DesktopEvent) => void,
): Promise<() => Promise<void>> {
  const channel = new Channel<DesktopEvent>()
  channel.onmessage = handler
  const id = await call<string>('events_subscribe', { channel })
  return () => call<void>('events_unsubscribe', { subscriptionId: id })
}

// ─── Projects ────────────────────────────────────────────────────────────────

export const projectsList = () => call<Project[]>('projects_list')
export const projectAdd = (path: string) => call<Project>('project_add', { path })
export const projectRemove = (projectId: string) => call<void>('project_remove', { projectId })
export const projectRename = (projectId: string, name: string) =>
  call<Project>('project_rename', { projectId, name })

// ─── Conversations ───────────────────────────────────────────────────────────

export const sessionsList = (projectId: string) =>
  call<SessionItem[]>('sessions_list', { projectId })
export const sessionsSearch = (projectId: string | null, query: string) =>
  call<SessionSearchHit[]>('sessions_search', { projectId, query })
export const sessionOpen = (request: OpenSessionRequest) =>
  call<OpenedSession>('session_open', { request })
export const sessionClose = (sessionId: string, cancel = false) =>
  call<void>('session_close', { sessionId, cancel })
export const sessionRename = (sessionId: string, title: string) =>
  call<SessionItem>('session_rename', { sessionId, title })
export const sessionDelete = (sessionId: string) => call<void>('session_delete', { sessionId })
export const sessionCommand = (sessionId: string, command: Command) =>
  call<void>('session_command', { sessionId, command })
export const sessionSnapshot = (sessionId: string) =>
  call<Snapshot>('session_snapshot', { sessionId })
export const attachmentSave = (name: string, mediaType: string, dataBase64: string) =>
  call<SavedAttachment>('attachment_save', { name, mediaType, dataBase64 })

// ─── Change journal ──────────────────────────────────────────────────────────

export const changeLogList = (projectId: string, sessionId: string | null, limit?: number) =>
  call<ChangeRecord[]>('change_log_list', { projectId, sessionId, limit: limit ?? null })
export const changeLogDiff = (recordId: string, path: string) =>
  call<FileDiff>('change_log_diff', { recordId, path })
export const changeLogInfo = (projectId: string) =>
  call<ChangeJournalInfo>('change_log_info', { projectId })

// ─── Git ─────────────────────────────────────────────────────────────────────

export const gitStatus = (projectId: string) => call<GitStatus>('git_status', { projectId })
export const gitDiff = (projectId: string, path: string, area: GitArea) =>
  call<FileDiff>('git_diff', { projectId, path, area })
export const gitLog = (projectId: string, limit = 50, skip = 0) =>
  call<GitCommit[]>('git_log', { projectId, limit, skip })
export const gitShow = (projectId: string, rev: string) =>
  call<FileDiff[]>('git_show', { projectId, rev })

// ─── Terminal ────────────────────────────────────────────────────────────────

export async function ptyOpen(
  request: PtyOpenRequest,
  onEvent: (event: PtyEvent) => void,
): Promise<PtyInfo> {
  const channel = new Channel<PtyEvent>()
  channel.onmessage = onEvent
  return call<PtyInfo>('pty_open', { request, channel })
}
export const ptyWrite = (ptyId: string, data: string) => call<void>('pty_write', { ptyId, data })
export const ptyResize = (ptyId: string, cols: number, rows: number) =>
  call<void>('pty_resize', { ptyId, cols, rows })
export const ptyKill = (ptyId: string) => call<void>('pty_kill', { ptyId })
export const ptyList = (projectId: string | null) => call<PtyInfo[]>('pty_list', { projectId })

// ─── Paths ───────────────────────────────────────────────────────────────────

export const pathKind = (path: string) => call<'file' | 'folder' | 'missing'>('path_kind', { path })
