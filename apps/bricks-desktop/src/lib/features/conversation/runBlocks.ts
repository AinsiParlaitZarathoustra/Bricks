// The timeline as the person reads it: each run's steps (reasoning, tools,
// sub-agents, intermediate messages) are gathered in one work block under the
// message that started the run; once the run has ended, its last message with
// text is the answer and follows the block, then the run's end summary. Items
// without a run (an older history) stay as they are. Pure: unit-tested
// (tests/unit/run-blocks.test.ts).
import type { AssistantItem, RunInfo, TimelineItem } from '$lib/state/conversation.svelte'
import type { IconComponent } from '$lib/layout/types'
import { MESSAGE_ICON, THINKING_ICON, toolIcon } from './timeline/toolIcons'

export interface WorkBlock {
  kind: 'work'
  key: string
  runId: string
  /** The steps of the run, in order; never the answer, the user message or the end. */
  items: TimelineItem[]
  /** The answer's own reasoning, shown with the steps while the answer is apart. */
  answerReasoning: AssistantItem | null
}

export type TimelineBlock =
  | { kind: 'item'; key: string; item: TimelineItem; /** The run's answer: its text only. */ answer?: boolean }
  | WorkBlock

/** The last message with text of an ended run: its answer. `null` while it runs. */
export function answerOf(
  items: readonly TimelineItem[],
  runId: string,
  running: boolean,
): AssistantItem | null {
  if (running) return null
  for (let i = items.length - 1; i >= 0; i--) {
    const item = items[i]
    if (item.kind === 'assistant' && item.runId === runId && item.text.trim()) return item
  }
  return null
}

/**
 * The blocks of `visible` (a window of `all`). The answer is found in `all`, so a
 * window that starts inside a run still knows it. `runningRunId` is the run at work.
 */
export function timelineBlocks(
  visible: readonly TimelineItem[],
  all: readonly TimelineItem[],
  runs: ReadonlyMap<string, RunInfo>,
  runningRunId: string | null,
): TimelineBlock[] {
  // Runs of the window that have ended: their answers, found in one pass from the end.
  const wanted = new Set<string>()
  for (const item of visible) {
    if (item.runId && item.runId !== runningRunId && runs.has(item.runId)) wanted.add(item.runId)
  }
  const answers = new Map<string, AssistantItem>()
  for (let i = all.length - 1; i >= 0 && answers.size < wanted.size; i--) {
    const item = all[i]
    if (item.kind !== 'assistant' || !item.runId || !wanted.has(item.runId) || answers.has(item.runId)) continue
    if (item.text.trim()) answers.set(item.runId, item)
  }

  // The steps of each run in the window, in order.
  const steps = new Map<string, TimelineItem[]>()
  for (const item of visible) {
    const runId = item.runId
    if (!runId || !runs.has(runId) || item.kind === 'user' || item.kind === 'run_end') continue
    if (item === answers.get(runId)) continue
    let list = steps.get(runId)
    if (!list) steps.set(runId, (list = []))
    list.push(item)
  }

  const blocks: TimelineBlock[] = []
  const opened = new Set<string>()
  const open = (runId: string) => {
    opened.add(runId)
    const final = answers.get(runId)
    const reasoning = final && final.thinking.trim() && visible.includes(final) ? final : null
    blocks.push({ kind: 'work', key: `work-${runId}`, runId, items: steps.get(runId) ?? [], answerReasoning: reasoning })
  }

  for (const item of visible) {
    const runId = item.runId
    // A run the timeline knows nothing about keeps its items as they are.
    if (!runId || !runs.has(runId)) {
      blocks.push({ kind: 'item', key: item.id, item })
      continue
    }
    if (item.kind === 'user') {
      blocks.push({ kind: 'item', key: item.id, item })
      if (!opened.has(runId)) open(runId)
      continue
    }
    if (!opened.has(runId)) open(runId)
    if (item.kind === 'run_end') blocks.push({ kind: 'item', key: item.id, item })
    else if (item === answers.get(runId)) blocks.push({ kind: 'item', key: item.id, item, answer: true })
  }
  return blocks
}

/** Items that ask the person something: shown even when the steps are folded. */
export function needsAttention(item: TimelineItem): boolean {
  return (item.kind === 'approval' && !item.resolved) || (item.kind === 'question' && !item.outcome)
}

export interface CurrentStep {
  icon: IconComponent
  label: string
}

/** What the run is doing now: its last step that is a tool, a sub-agent or a message. */
export function currentStep(items: readonly TimelineItem[]): CurrentStep | null {
  for (let i = items.length - 1; i >= 0; i--) {
    const item = items[i]
    if (item.kind === 'tool') return { icon: toolIcon(item.name), label: item.name }
    if (item.kind === 'agent') return { icon: toolIcon('Agent'), label: 'Agent' }
    if (item.kind === 'assistant') {
      if (item.text.trim()) return { icon: MESSAGE_ICON, label: 'Rédige' }
      if (item.thinking.trim() || item.streaming) return { icon: THINKING_ICON, label: 'Réfléchit' }
    }
  }
  return null
}
