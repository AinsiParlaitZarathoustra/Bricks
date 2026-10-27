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
import type { MemoryCounts } from './bindings/MemoryCounts'
import type { MemoryEntryFilter } from './bindings/MemoryEntryFilter'
import type { MemoryEntryPage } from './bindings/MemoryEntryPage'
import type { MemoryOverview } from './bindings/MemoryOverview'
import type { MemoryScope } from './bindings/MemoryScope'
import type { OpenedSession } from './bindings/OpenedSession'
import type { OpenSessionRequest } from './bindings/OpenSessionRequest'
import type { Preferences } from './bindings/Preferences'
import type { Project } from './bindings/Project'
import type { PtyEvent } from './bindings/PtyEvent'
import type { PtyInfo } from './bindings/PtyInfo'
import type { PtyOpenRequest } from './bindings/PtyOpenRequest'
import type { SavedAttachment } from './bindings/SavedAttachment'
import type { AttachmentScope } from './bindings/AttachmentScope'
import type { AttachmentRef } from './bindings/AttachmentRef'
import type { AttachmentPreview } from './bindings/AttachmentPreview'
import type { ImportedAttachment } from './bindings/ImportedAttachment'
import type { ResolvedAttachment } from './bindings/ResolvedAttachment'
import type { SessionItem } from './bindings/SessionItem'
import type { SessionSearchHit } from './bindings/SessionSearchHit'
import type { ProjectFileWindow } from './bindings/ProjectFileWindow'
import type { PermissionsDefault } from './bindings/PermissionsDefault'
import type { Command, ModelChoice, SessionPolicy, Snapshot } from './engine'

export type * from './bindings/AppIcon'
export type * from './bindings/AskTimerMode'
export type * from './bindings/AskTimerPrefs'
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
export type * from './bindings/MemoryCounts'
export type * from './bindings/MemoryEntry'
export type * from './bindings/MemoryEntryFilter'
export type * from './bindings/MemoryEntryKind'
export type * from './bindings/MemoryEntryPage'
export type * from './bindings/MemoryEvidence'
export type * from './bindings/MemoryOverview'
export type * from './bindings/MemoryScope'
export type * from './bindings/RetiredMemory'
export type * from './bindings/OpenedSession'
export type * from './bindings/OpenSessionRequest'
export type * from './bindings/Preferences'
export type * from './bindings/Project'
export type * from './bindings/ProviderInfo'
export type * from './bindings/PtyEvent'
export type * from './bindings/PtyInfo'
export type * from './bindings/PtyOpenRequest'
export type * from './bindings/SavedAttachment'
export type * from './bindings/AttachmentKind'
export type * from './bindings/AttachmentScope'
export type * from './bindings/AttachmentRef'
export type * from './bindings/AttachmentPreview'
export type * from './bindings/ImportedAttachment'
export type * from './bindings/ResolvedAttachment'
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
/** Shows the light or the dark icon in the Dock (macOS). */
export const appIconShow = (dark: boolean) => call<void>('app_icon_show', { dark })
export const permissionsDefaultGet = () => call<PermissionsDefault>('permissions_default_get')
export const permissionsDefaultSet = (policy: SessionPolicy) =>
  call<void>('permissions_default_set', { policy })

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
/** Moves a project to another folder; its conversations follow it. */
export const projectRelocate = (projectId: string, path: string) =>
  call<Project>('project_relocate', { projectId, path })

// ─── Memory ──────────────────────────────────────────────────────────────────

/** Counts of both memories, notices and the stores of removed projects. */
export const memoryOverview = (projectId: string | null) =>
  call<MemoryOverview>('memory_overview', { projectId })
export const memoryCounts = (scope: MemoryScope, projectId: string | null) =>
  call<MemoryCounts>('memory_counts', { scope, projectId })
/** A page of entries (`limit` 1–50), newest first. */
export const memoryEntries = (
  scope: MemoryScope,
  projectId: string | null,
  filter: MemoryEntryFilter,
  query: string,
  offset: number,
  limit: number,
) => call<MemoryEntryPage>('memory_entries', { scope, projectId, filter, query, offset, limit })
/** Clears a memory; refused while a run uses it. */
export const memoryDelete = (scope: MemoryScope, projectId: string | null) =>
  call<void>('memory_delete', { scope, projectId })
export const memoryDeleteRetired = (projectId: string) =>
  call<void>('memory_delete_retired', { projectId })

// ─── Project files (references of messages, SVG export) ─────────────────────

/** Read-only window of a project file: `line` alone centres a window on it, with `endLine` a range. */
export const projectFileRead = (
  projectId: string,
  path: string,
  line: number | null = null,
  endLine: number | null = null,
) => call<ProjectFileWindow>('project_file_read', { projectId, path, line, endLine })
/** Native save dialog for an SVG document: the path written, or null when cancelled. */
export const saveSvgFile = (defaultName: string, contents: string) =>
  call<string | null>('save_svg_file', { defaultName, contents })
/**
 * Reveals in the Finder a file a sub-agent changed, resolved in that agent's
 * recorded workspace (shared project or isolated worktree) by Rust; a deleted
 * file reveals its nearest existing folder. Nothing is opened or run.
 */
export const revealAgentFile = (sessionId: string, agentId: string, path: string) =>
  call<void>('reveal_agent_file', { sessionId, agentId, path })

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
/** `display`: the typed command of a submit (e.g. `/skill debug`), shown on its user message. */
export const sessionCommand = (sessionId: string, command: Command, display: string | null = null) =>
  call<void>('session_command', { sessionId, command, display })
export const sessionSnapshot = (sessionId: string) =>
  call<Snapshot>('session_snapshot', { sessionId })
/** Compatibility: pasted bytes as a draft copy, by its absolute path. Use `attachmentImportBytes`. */
export const attachmentSave = (name: string, mediaType: string, dataBase64: string) =>
  call<SavedAttachment>('attachment_save', { name, mediaType, dataBase64 })

// ─── Attachment copies (drafts, sessions) ────────────────────────────────────
// The desktop copies each attachment at import and keeps the copy under an
// opaque id. The interface never chooses a destination: it names drafts and
// sessions by id, and attachments by id. The type is found from the content.

/** Imports pasted bytes (base64) into `draftId`, or into a new draft when it is null. */
export const attachmentImportBytes = (draftId: string | null, name: string, dataBase64: string) =>
  call<ImportedAttachment>('attachment_import_bytes', { draftId, name, dataBase64 })
/** Imports the file at an absolute `path` into `draftId`, or into a new draft when it is null. */
export const attachmentImportPath = (draftId: string | null, path: string) =>
  call<ImportedAttachment>('attachment_import_path', { draftId, path })
/** Frees the copy of a draft. */
export const discardDraftAttachment = (draftId: string, attachmentId: string) =>
  call<void>('discard_draft_attachment', { draftId, attachmentId })
/** Removes a copy of a session, refused when a sent message refers to it. */
export const discardSessionAttachment = (sessionId: string, attachmentId: string) =>
  call<void>('discard_session_attachment', { sessionId, attachmentId })
/** Removes a draft and its copies. */
export const discardDraft = (draftId: string) => call<void>('discard_draft', { draftId })
/** Moves a draft's copies into the session, before its submit. */
export const promoteDraft = (draftId: string, sessionId: string) =>
  call<AttachmentRef[]>('promote_draft', { draftId, sessionId })
/** Absolute paths of a session's copies, in the order given: what a submit sends. */
/** The image a tool call showed the model, as a data URL; null when it showed none. */
export const sessionToolImage = (sessionId: string, toolCallId: string) =>
  call<string | null>('session_tool_image', { sessionId, toolCallId })
/** The index of the user message a finished run answered (what `revise` names); null while unknown. */
export const sessionRunUserIndex = (sessionId: string, runId: string) =>
  call<number | null>('session_run_user_index', { sessionId, runId })
export const sessionAttachmentPaths = (sessionId: string, attachmentIds: string[]) =>
  call<ResolvedAttachment[]>('session_attachment_paths', { sessionId, attachmentIds })
/** A copy for display: an image as a data URL, a text as its beginning. */
export const attachmentPreview = (scope: AttachmentScope, ownerId: string, attachmentId: string) =>
  call<AttachmentPreview>('attachment_preview', { scope, ownerId, attachmentId })

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

// ─── Paths ───────────────────────────────────────────────────────────────────

export const pathKind = (path: string) => call<'file' | 'folder' | 'missing'>('path_kind', { path })

// ─── slash commands and inspection ──────────────────────────────

export type * from './bindings/SlashCommandInfo'
export type * from './bindings/SlashAction'
export type * from './bindings/SlashPresent'
export type * from './bindings/SessionToolInfo'
export type * from './bindings/McpServerInfo'
export type * from './bindings/SessionConfigInfo'
export type * from './bindings/ConfigEntry'
import type { SlashCommandInfo } from './bindings/SlashCommandInfo'
import type { SlashAction } from './bindings/SlashAction'
import type { SessionToolInfo } from './bindings/SessionToolInfo'
import type { McpServerInfo } from './bindings/McpServerInfo'
import type { SessionConfigInfo } from './bindings/SessionConfigInfo'

export const slashCommands = () => call<SlashCommandInfo[]>('slash_commands')
/** Throws `IpcError` (`invalid`) with a French message for a bad line. */
export const slashParse = (line: string) => call<SlashAction>('slash_parse', { line })
export const sessionTools = (sessionId: string) =>
  call<SessionToolInfo[]>('session_tools', { sessionId })
export const sessionMcp = (sessionId: string) => call<McpServerInfo[]>('session_mcp', { sessionId })
export const sessionConfig = (sessionId: string) =>
  call<SessionConfigInfo>('session_config', { sessionId })
export const sessionLocate = (sessionId: string) =>
  call<SessionItem>('session_locate', { sessionId })
export const appQuit = () => call<void>('app_quit')

export type * from './bindings/ProjectFileEntry'
import type { ProjectFileEntry } from './bindings/ProjectFileEntry'
/** `@` mentions: fuzzy-matched project files (best first). */
export const projectFiles = (projectId: string, query: string, limit = 50, refresh = false) =>
  call<ProjectFileEntry[]>('project_files', { projectId, query, limit, refresh })

// ─── provider/model editing ─────────────────────────────────────

export type * from './bindings/PermissionsDefault'
export type * from './bindings/ProvidersFile'
export type * from './bindings/ProviderView'
export type * from './bindings/ModelView'
export type * from './bindings/ModelProfileView'
export type * from './bindings/ProviderPatch'
export type * from './bindings/ModelPatch'
export type * from './bindings/PricingView'
export type * from './bindings/PricingEdit'
import type { ProvidersFile } from './bindings/ProvidersFile'
import type { ProviderPatch } from './bindings/ProviderPatch'
import type { ModelPatch } from './bindings/ModelPatch'

/** The providers file as loaded (no secret). */
export const providersConfig = () => call<ProvidersFile>('providers_config')
/** Each write needs the `revision` read last; `conflict` if the file changed. */
export const providerUpdate = (providerId: string, patch: ProviderPatch, revision: string) =>
  call<ProvidersFile>('provider_update', { providerId, patch, revision })
export const modelUpdate = (providerId: string, modelId: string, patch: ModelPatch, revision: string) =>
  call<ProvidersFile>('model_update', { providerId, modelId, patch, revision })
export const providerDelete = (providerId: string, revision: string) =>
  call<ProvidersFile>('provider_delete', { providerId, revision })
export const modelDelete = (providerId: string, modelId: string, revision: string) =>
  call<ProvidersFile>('model_delete', { providerId, modelId, revision })
export const providerCreate = (providerId: string, patch: ProviderPatch, revision: string) =>
  call<ProvidersFile>('provider_create', { providerId, patch, revision })
export const modelCreate = (providerId: string, modelId: string, patch: ModelPatch, revision: string) =>
  call<ProvidersFile>('model_create', { providerId, modelId, patch, revision })
/** Writes 0 for every missing input, output and cache-read price, in one edit. */
export const providersFillMissingPrices = (revision: string) =>
  call<ProvidersFile>('providers_fill_missing_prices', { revision })

// ─── skills catalogue, colours, reasoning profiles ──────────────

export type * from './bindings/ThemeColors'
export type * from './bindings/ColorSet'
export type * from './bindings/ReasoningProfileEdit'
export type * from './bindings/SkillsCatalog'
export type * from './bindings/SkillSourceView'
export type * from './bindings/SkillChange'
export type * from './bindings/SkillProblem'
import type { SkillChange } from './bindings/SkillChange'
import type { SkillsCatalog } from './bindings/SkillsCatalog'

/** Settings: the skills of Bricks' store and the built-in ones. */
export const skillsCatalog = (projectId: string | null, query = '', page = 0, reload = false) =>
  call<SkillsCatalog>('skills_catalog', { projectId, query, page, reload })
/** Settings: writes a skill into the store. A refusal is `problem`, not a rejection. */
export const skillsCreate = (name: string, description: string, instructions: string) =>
  call<SkillChange>('skills_create', { name, description, instructions })
/** Settings: copies the skill folder `path` (chosen with the dialog) into the store. */
export const skillsImport = (path: string) => call<SkillChange>('skills_import', { path })
/** Settings: removes an installed skill, by name. */
export const skillsRemove = (name: string) => call<SkillChange>('skills_remove', { name })

/** Sub-agent instance records of an open session (engine `InstanceRecord[]`). */
export const sessionAgents = (sessionId: string) =>
  call<import('./engine').InstanceRecord[]>('session_agents', { sessionId })
/** Isolated ChangeSets of an open session (engine `ChangeSet[]`). */
export const sessionChangesets = (sessionId: string) =>
  call<import('./engine').ChangeSet[]>('session_changesets', { sessionId })

export type * from './bindings/RunLabel'
export type * from './bindings/RunRecord'
export type * from './bindings/RunFile'
export type * from './bindings/RunCommand'
