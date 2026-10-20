// The responses of a conversation: what the person copies with « Copier », and where
// that button goes. A response is the items of one run (or, for a stored history
// without run ids, the items after one user message). Pure: unit-tested
// (tests/unit/responses.test.ts).
import type { RunRecord } from '$lib/ipc/client'
import type { RunInfo, TimelineItem } from '$lib/state/conversation.svelte'

export interface ResponseGroup {
  /** Stable within a conversation: the run id, else the user item's id. */
  key: string
  /** The user message that asked for it (`null` when the history does not hold it). */
  userItemId: string | null
  runId: string | null
  /** The assistant items of the response, in order. */
  assistantItemIds: string[]
  /** The last item of the response: the copy button goes right after it. `null` when empty. */
  anchorItemId: string | null
  /** The run ended (or the stored history, which only holds ended runs). */
  complete: boolean
  /**
   * The answer's text: its text segments in order (each trimmed, empty ones left
   * out), joined by a blank line. No reasoning, tool, notice or summary. A stored
   * run with a record uses the record's text; otherwise the items'.
   */
  copyText: string
}

interface Open {
  userItemId: string | null
  runId: string | null
  items: TimelineItem[]
  assistants: string[]
  ended: boolean
}

/**
 * The response groups of a conversation, in order. Built from every item (not the
 * visible ones). A group starts at a user item, or at an item of another run.
 */
export function responseGroups(
  items: readonly TimelineItem[],
  runs: ReadonlyMap<string, RunInfo>,
  runRecords: readonly RunRecord[] = [],
): ResponseGroup[] {
  const records = new Map(runRecords.map((r) => [r.run_id, r]))
  const groups: ResponseGroup[] = []
  const state: { open: Open | null } = { open: null }

  const close = () => {
    if (state.open) groups.push(finish(state.open, groups.length, runs, records))
    state.open = null
  }

  for (const item of items) {
    if (item.kind === 'user') {
      close()
      state.open = { userItemId: item.id, runId: item.runId ?? null, items: [], assistants: [], ended: false }
      continue
    }
    if (item.kind === 'run_end') {
      if (state.open?.runId === item.runId) state.open.ended = true
      continue
    }
    const current = state.open
    if (item.runId && current?.runId && current.runId !== item.runId) close()
    if (!state.open) state.open = { userItemId: null, runId: item.runId ?? null, items: [], assistants: [], ended: false }
    const group = state.open
    if (item.runId && !group.runId) group.runId = item.runId
    group.items.push(item)
    if (item.kind === 'assistant' && item.text.trim()) group.assistants.push(item.text.trim())
  }
  close()
  return groups
}

function finish(
  open: Open,
  index: number,
  runs: ReadonlyMap<string, RunInfo>,
  records: ReadonlyMap<string, RunRecord>,
): ResponseGroup {
  const run = open.runId ? runs.get(open.runId) : undefined
  const record = open.runId ? records.get(open.runId) : undefined
  const complete =
    open.runId === null || open.ended || run?.finishedAt !== undefined || record !== undefined
  const copyText = record ? record.assistant_text : open.assistants.join('\n\n')
  const last = open.items.at(-1)
  return {
    key: open.runId ?? (open.userItemId ? `user:${open.userItemId}` : `group:${index}`),
    userItemId: open.userItemId,
    runId: open.runId,
    assistantItemIds: open.items.filter((i) => i.kind === 'assistant').map((i) => i.id),
    anchorItemId: last?.id ?? null,
    complete,
    copyText,
  }
}

/** Whether the « Copier » button is offered for a response: complete and not empty. */
export function isCopyable(group: ResponseGroup): boolean {
  return group.complete && group.copyText.trim() !== ''
}

/** The copyable response that ends at an item, if any. */
export function responseByAnchor(groups: readonly ResponseGroup[], itemId: string): ResponseGroup | undefined {
  return groups.find((g) => g.anchorItemId === itemId && isCopyable(g))
}
