// Read-only windows of the files a conversation names, for the reader of the
// right column. Rust resolves the path (in the workspace of the agent, or in the
// project folder of the session), refuses what leaves that workspace, and
// returns the window with the file's canonical absolute path.
// Owner: logique 2. `client.ts` keeps its `call` private, so the error mapping
// is repeated here (an `IpcError` carries the Rust code and message).

import { invoke } from '@tauri-apps/api/core'
import type { IpcErrorCode } from './bindings/IpcErrorCode'
import type { ProjectFileWindow } from './bindings/ProjectFileWindow'
import { IpcError } from './client'

/** A window read by the reader: the `project_file_read` window and the file's canonical absolute path. */
export type ReaderFileWindow = ProjectFileWindow & { absolute: string }

export interface ReaderFileRequest {
  sessionId: string
  /** The agent whose workspace holds the file; `null` for the project folder of the session. */
  agentId: string | null
  /** Relative to that workspace (no `..`), or absolute inside it. */
  path: string
  /** 1-based. Alone: a window centred on it. With `endLine`: the range, held by a window of 400 lines. */
  line: number | null
  endLine: number | null
}

async function call<T>(cmd: string, args: Record<string, unknown>): Promise<T> {
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

/** Reads a window of a file of the open session `request.sessionId` (see `ReaderFileRequest`). */
export const readerFileRead = (request: ReaderFileRequest) =>
  call<ReaderFileWindow>('reader_file_read', {
    sessionId: request.sessionId,
    agentId: request.agentId,
    path: request.path,
    line: request.line,
    endLine: request.endLine,
  })

/**
 * Reveals in the Finder a file of the reader of the open session, resolved like
 * `readerFileRead`; a deleted file reveals its nearest existing folder.
 */
export const readerFileReveal = (sessionId: string, agentId: string | null, path: string) =>
  call<void>('reader_file_reveal', { sessionId, agentId, path })
