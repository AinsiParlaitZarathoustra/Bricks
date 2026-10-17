// Edits of one conversation, grouped by run and by file.
//
// Two sources, one per edit: the live timeline (`edit` items, seen since the
// app started) and the change journal (every record of the session, including
// earlier app runs). A tool call present in both counts once, from the journal
// (it carries the patches). Counts are sums of edit operations, not a net diff.

import type { ChangeRecord } from '$lib/ipc/client'
import type { TimelineItem } from '$lib/state/conversation.svelte'

export interface OperationView {
  /** Tool call id: identifies one application in the journal. */
  key: string
  runId: string | null
  /** Journal time; `null` for an edit not yet in the journal. */
  at: number | null
  /** Journal record holding the patch, when the journal has one. */
  recordId: string | null
  path: string
  kind: string
  added: number
  removed: number
  binary: boolean
  /** A patch can be fetched with `changeLogDiff(recordId, path)`. */
  hasPatch: boolean
  agentId: string | null
  changesetId: string | null
}

export interface FileView {
  path: string
  /** Net kind: created during the run stays `create` unless deleted later. */
  kind: string
  added: number
  removed: number
  binary: boolean
  agents: string[]
  operations: OperationView[]
}

export interface RunGroup {
  runId: string | null
  startedAt: number | null
  files: FileView[]
}

export function conversationChanges(
  items: readonly TimelineItem[],
  records: readonly ChangeRecord[],
  runStart: (runId: string) => number | undefined,
): RunGroup[] {
  const operations: OperationView[] = []
  const counted = new Set<string>()

  for (const record of records) {
    if (counted.has(record.tool_call_id)) continue
    counted.add(record.tool_call_id)
    for (const file of record.files) {
      operations.push({
        key: record.tool_call_id,
        runId: record.run_id,
        at: record.at,
        recordId: record.id,
        path: file.path,
        kind: file.kind,
        added: file.added,
        removed: file.removed,
        binary: file.binary,
        hasPatch: file.has_patch,
        agentId: record.agent_id,
        changesetId: record.changeset_id,
      })
    }
  }

  for (const item of items) {
    if (item.kind !== 'edit' || counted.has(item.toolCallId)) continue
    counted.add(item.toolCallId)
    for (const file of item.files) {
      operations.push({
        key: item.toolCallId,
        runId: item.runId ?? null,
        at: null,
        recordId: null,
        path: file.path,
        kind: file.kind,
        added: file.added,
        removed: file.removed,
        binary: file.binary ?? false,
        hasPatch: false,
        agentId: item.agentId ?? null,
        changesetId: item.changesetId ?? null,
      })
    }
  }

  const byRun = new Map<string | null, OperationView[]>()
  for (const op of operations) {
    const list = byRun.get(op.runId)
    if (list) list.push(op)
    else byRun.set(op.runId, [op])
  }

  const groups: RunGroup[] = []
  for (const [runId, list] of byRun) {
    groups.push({
      runId,
      startedAt: startOf(runId, list, runStart),
      files: fileViews(list),
    })
  }
  return groups.sort((a, b) => (b.startedAt ?? 0) - (a.startedAt ?? 0))
}

function startOf(
  runId: string | null,
  list: readonly OperationView[],
  runStart: (runId: string) => number | undefined,
): number | null {
  const fromRun = runId ? runStart(runId) : undefined
  if (fromRun !== undefined) return fromRun
  let earliest: number | null = null
  for (const op of list) {
    if (op.at !== null && (earliest === null || op.at < earliest)) earliest = op.at
  }
  return earliest
}

function fileViews(list: readonly OperationView[]): FileView[] {
  const byPath = new Map<string, FileView>()
  for (const op of list) {
    let file = byPath.get(op.path)
    if (!file) {
      file = { path: op.path, kind: op.kind, added: 0, removed: 0, binary: false, agents: [], operations: [] }
      byPath.set(op.path, file)
    }
    file.added += op.added
    file.removed += op.removed
    file.binary ||= op.binary
    if (op.agentId && !file.agents.includes(op.agentId)) file.agents.push(op.agentId)
    file.operations.push(op)
  }
  for (const file of byPath.values()) {
    const first = file.operations[0].kind
    const last = file.operations[file.operations.length - 1].kind
    file.kind = first === 'create' && last !== 'delete' ? 'create' : last
  }
  return [...byPath.values()].sort((a, b) => a.path.localeCompare(b.path, 'fr'))
}
