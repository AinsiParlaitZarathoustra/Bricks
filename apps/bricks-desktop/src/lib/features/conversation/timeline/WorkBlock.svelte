<script lang="ts">
  // The steps of one run under a thin bar. While the run works, the bar counts
  // its time and the steps are shown; folded, the bar names the current step.
  // Once the run has ended the bar reads « A travaillé … » and folds by itself:
  // the answer follows it. What asks the person something stays shown folded.
  import ChevronRight from '@lucide/svelte/icons/chevron-right'
  import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '$lib/components/ui/collapsible'
  import type { AssistantItem, Conversation, TimelineItem } from '$lib/state/conversation.svelte'
  import type { RunOutcome } from '$lib/ipc/engine'
  import { OUTCOME_LABEL } from '../format'
  import { currentStep, needsAttention } from '../runBlocks'
  import { formatWorked } from '../runend'
  import AssistantMessage from './AssistantMessage.svelte'
  import TimelineEntry from './TimelineEntry.svelte'
  import WorkingLine from './WorkingLine.svelte'

  interface Props {
    conversation: Conversation
    runId: string
    items: TimelineItem[]
    answerReasoning?: AssistantItem | null
  }

  let { conversation, runId, items, answerReasoning = null }: Props = $props()

  const running = $derived(conversation.runningRunId === runId)
  const run = $derived(conversation.runs.get(runId))
  const worked = $derived(formatWorked(run?.workedMs ?? null))
  const outcome = $derived<RunOutcome | null>(run?.outcome ?? null)
  const hasSteps = $derived(items.length > 0 || answerReasoning !== null)
  const step = $derived(running ? currentStep(items) : null)
  const attention = $derived(items.filter(needsAttention))

  // Shown while the run works, folded once it ends; the person's own toggle holds until then.
  let open = $derived(running)

  const TONE: Record<RunOutcome, string> = {
    succeeded: '',
    incomplete: 'text-warning',
    failed: 'text-destructive',
    cancelled: '',
  }
</script>

{#snippet bar()}
  {#if running}
    <WorkingLine {conversation} />
  {:else}
    <span class="tabular-nums">A travaillé{worked ? ` ${worked}` : ''}</span>
    {#if outcome && outcome !== 'succeeded'}
      <span aria-hidden="true">·</span>
      <span class={TONE[outcome]}>{OUTCOME_LABEL[outcome]}</span>
    {/if}
  {/if}
{/snippet}

<Collapsible bind:open class="flex flex-col gap-2">
  <div class="flex min-w-0 items-center gap-3 text-ui text-muted-foreground">
    {#if hasSteps}
      <CollapsibleTrigger
        class="group inline-flex shrink-0 items-center gap-1.5 rounded-md hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
      >
        {@render bar()}
        <ChevronRight class="size-3.5 transition-transform group-data-[state=open]:rotate-90" aria-hidden="true" />
      </CollapsibleTrigger>
    {:else}
      <p class="inline-flex shrink-0 items-center gap-1.5">{@render bar()}</p>
    {/if}
    {#if step && !open}
      <span class="inline-flex min-w-0 items-center gap-1.5 text-xs">
        <step.icon class="size-3.5 shrink-0 animate-pulse" aria-hidden="true" />
        <span class="min-w-0 truncate">{step.label}</span>
      </span>
    {/if}
  </div>
  <div aria-hidden="true" class="h-px bg-border"></div>

  {#if !open}
    {#each attention as item (item.id)}
      <TimelineEntry {item} {conversation} />
    {/each}
  {/if}

  <CollapsibleContent class="flex flex-col gap-2">
    {#each items as item (item.id)}
      <TimelineEntry {item} {conversation} />
    {/each}
    {#if answerReasoning}
      <AssistantMessage item={answerReasoning} {conversation} part="reasoning" />
    {/if}
  </CollapsibleContent>
</Collapsible>
