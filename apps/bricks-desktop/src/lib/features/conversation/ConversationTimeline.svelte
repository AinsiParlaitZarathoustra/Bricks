<script lang="ts">
  import { tick, untrack } from 'svelte'
  import { ArrowDown } from '@lucide/svelte'
  import { Button } from '$lib/components/ui/button'
  import type { Conversation, TimelineItem } from '$lib/state/conversation.svelte'
  import { OUTCOME_LABEL, plural, truncate } from './format'
  import ApprovalCard from './timeline/ApprovalCard.svelte'
  import QuestionCard from './timeline/QuestionCard.svelte'
  import SkillsTrustBanner from './timeline/SkillsTrustBanner.svelte'
  import TimelineEntry from './timeline/TimelineEntry.svelte'
  import WorkBlock from './timeline/WorkBlock.svelte'
  import { timelineBlocks } from './runBlocks'

  let { conversation }: { conversation: Conversation } = $props()

  /** Items rendered at once; earlier ones load on request. */
  const WINDOW = 200
  const NEAR_BOTTOM_PX = 80

  let scrollEl = $state<HTMLDivElement | null>(null)
  let listEl = $state<HTMLDivElement | null>(null)
  let nearBottom = $state(true)
  /** Items that arrived while the user was scrolled up. */
  let unseen = $state(0)
  let limit = $state(WINDOW)
  /** Text of the polite live region: completed messages and attention needs only. */
  let announcement = $state('')

  const hiddenCount = $derived(Math.max(0, conversation.items.length - limit))
  const visible = $derived(hiddenCount > 0 ? conversation.items.slice(hiddenCount) : conversation.items)
  const blocks = $derived(
    timelineBlocks(visible, conversation.items, conversation.runs, conversation.runningRunId),
  )
  /** The run at work has no block when none of its items is in the view (it started elsewhere). */
  const runningWithoutBlock = $derived(
    conversation.runningRunId !== null &&
      !blocks.some((block) => block.kind === 'work' && block.runId === conversation.runningRunId),
  )

  /** Waiting approvals with no card in the timeline (e.g. after a reload). */
  const orphanApprovals = $derived(
    conversation.pendingApprovals.filter(
      (approval) =>
        !conversation.items.some(
          (item) => item.kind === 'approval' && item.approval.approval_id === approval.approval_id,
        ),
    ),
  )

  /** Waiting questions with no card in the timeline (e.g. after a reload). */
  const orphanQuestions = $derived(
    conversation.pendingQuestions.filter(
      (question) =>
        !conversation.items.some(
          (item) => item.kind === 'question' && item.question.question_id === question.question_id,
        ),
    ),
  )

  /** Length of the growing text of the last item: changes on every streamed delta. */
  const tail = $derived.by(() => {
    const last = conversation.items.at(-1)
    if (!last) return 0
    if (last.kind === 'assistant') return last.text.length + last.thinking.length
    if (last.kind === 'tool') return (last.output?.length ?? 0) + (last.progress?.length ?? 0)
    return 0
  })

  function updateNearBottom() {
    const el = scrollEl
    if (!el) return
    nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight <= NEAR_BOTTOM_PX
    if (nearBottom) unseen = 0
  }

  function scrollToBottom(smooth = false) {
    const el = scrollEl
    if (!el) return
    el.scrollTo({ top: el.scrollHeight, behavior: smooth ? 'smooth' : 'auto' })
    nearBottom = true
    unseen = 0
  }

  async function loadEarlier() {
    const el = scrollEl
    const before = el?.scrollHeight ?? 0
    limit += WINDOW
    await tick()
    // Keep the reader on the same message when older ones are added above.
    if (el) el.scrollTop += el.scrollHeight - before
  }

  // Follow the output while the run streams, only if the user is at the bottom.
  // Opening a conversation starts at the bottom; a new item sent by the user too.
  let prevCount = -1
  let initialised = false
  $effect(() => {
    // Read outside untrack so the effect runs again once the element is bound.
    const el = scrollEl
    const count = conversation.items.length
    void tail
    const running = conversation.running
    void conversation.skillsTrust
    void orphanApprovals.length
    const grew = prevCount < 0 ? 0 : count - prevCount
    prevCount = count
    untrack(() => {
      if (!el) return
      if (!initialised) {
        initialised = true
        scrollToBottom()
        return
      }
      if (nearBottom) {
        if (running || grew > 0) scrollToBottom()
      } else if (grew > 0) {
        unseen += grew
      }
    })
  })

  // Panel resizes: stay at the bottom if we were there. While a run streams,
  // the rendered answer grows after the text changed (throttled rendering):
  // a reader following the bottom stays there; only their own scroll stops
  // it. Content that grows without a run (e.g. a diagram rendered, a tool row
  // expanded) only updates the state.
  $effect(() => {
    const scroller = scrollEl
    const list = listEl
    if (!scroller || !list) return
    const containerObserver = new ResizeObserver(() => {
      if (nearBottom) scrollToBottom()
    })
    const listObserver = new ResizeObserver(() => {
      if (nearBottom && conversation.running) scrollToBottom()
      else updateNearBottom()
    })
    containerObserver.observe(scroller)
    listObserver.observe(list)
    return () => {
      containerObserver.disconnect()
      listObserver.disconnect()
    }
  })

  function announcementFor(item: TimelineItem): string | null {
    switch (item.kind) {
      case 'approval':
        return item.resolved ? null : `Autorisation demandée : ${item.approval.tool}.`
      case 'question':
        return item.outcome ? null : `Question : ${item.question.question}`
      case 'run_end': {
        const run = conversation.runs.get(item.runId)
        const label = run?.outcome ? OUTCOME_LABEL[run.outcome] : 'fin de l’exécution'
        const text = run?.finalText?.trim()
        return text ? `Réponse terminée (${label}) : ${truncate(text, 300)}` : `Exécution terminée : ${label}.`
      }
      case 'notice':
        return item.level === 'error' ? item.message : null
      default:
        return null
    }
  }

  // Items appended after the view opened (not the stored history).
  let announcedCount = -1
  $effect(() => {
    const items = conversation.items
    const from = announcedCount
    announcedCount = items.length
    if (from < 0 || items.length <= from) return
    untrack(() => {
      for (const item of items.slice(from)) {
        const text = announcementFor(item)
        if (text) announcement = text
      }
    })
  })
</script>

<div class="relative flex h-full min-h-0 flex-1 flex-col">
  <!-- The timeline scrolls: keyboard users must be able to focus it (WCAG 2.1.1). -->
  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <div
    bind:this={scrollEl}
    data-conversation-scroll
    onscroll={updateNearBottom}
    role="region"
    aria-label="Conversation"
    tabindex="0"
    class="edge-fade min-h-0 flex-1 overflow-y-auto overscroll-contain focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring focus-visible:outline-none"
  >
    <div
      bind:this={listEl}
      role="log"
      aria-live="off"
      aria-label="Échanges de la conversation"
      class="mx-auto flex w-full max-w-3xl flex-col gap-6 px-4 pt-6 pb-10"
    >
      {#if hiddenCount > 0}
        <div class="flex justify-center">
          <Button variant="ghost" size="sm" onclick={loadEarlier}>
            Charger les messages précédents ({hiddenCount})
          </Button>
        </div>
      {/if}

      {#each blocks as block (block.key)}
        {#if block.kind === 'work'}
          <WorkBlock {conversation} runId={block.runId} items={block.items} answerReasoning={block.answerReasoning} />
        {:else}
          <TimelineEntry item={block.item} {conversation} answer={block.answer} />
        {/if}
      {/each}

      {#each orphanApprovals as approval (approval.approval_id)}
        <ApprovalCard {approval} {conversation} />
      {/each}

      {#each orphanQuestions as question (question.question_id)}
        <QuestionCard {question} {conversation} />
      {/each}

      <!-- A running run with nothing in the view (it started elsewhere): its bar goes last. -->
      {#if runningWithoutBlock && conversation.runningRunId}
        <WorkBlock {conversation} runId={conversation.runningRunId} items={[]} />
      {/if}

      {#if conversation.skillsTrust}
        <SkillsTrustBanner {conversation} trust={conversation.skillsTrust} />
      {/if}
    </div>
  </div>

  {#if !nearBottom}
    <div class="pointer-events-none absolute inset-x-0 bottom-3 flex justify-center">
      <Button
        variant="secondary"
        size="sm"
        class="pointer-events-auto shadow-md"
        onclick={() => scrollToBottom(true)}
      >
        <ArrowDown aria-hidden="true" />
        Aller en bas
        {#if unseen > 0}
          <span class="tabular-nums">· {unseen} {unseen > 1 ? 'nouveaux' : 'nouveau'}</span>
        {/if}
      </Button>
    </div>
  {/if}

  <p class="sr-only" role="status">{announcement}</p>
</div>

<style>
  /* Text scrolling under the top edge, or down towards the composer, fades out
     instead of being cut. Each fade covers the list's padding on its side, so
     the first and the last message are never dimmed at rest. */
  .edge-fade {
    --fade-top: 24px;
    --fade-bottom: 40px;
    -webkit-mask-image: linear-gradient(
      to bottom,
      transparent,
      #000 var(--fade-top),
      #000 calc(100% - var(--fade-bottom)),
      transparent
    );
    mask-image: linear-gradient(
      to bottom,
      transparent,
      #000 var(--fade-top),
      #000 calc(100% - var(--fade-bottom)),
      transparent
    );
  }
</style>
