<script lang="ts">
  // The session's plan above the composer, as a trace: one line for the plan
  // (its progress), and under it one line per step. While the conversation works,
  // the plan and its current step shimmer; steps still to do are dimmed.
  import { t } from '$lib/i18n/i18n.svelte'
  import Circle from '@lucide/svelte/icons/circle'
  import CircleCheck from '@lucide/svelte/icons/circle-check'
  import CircleDot from '@lucide/svelte/icons/circle-dot'
  import ListCheck from '@lucide/svelte/icons/list-check'
  import * as Trace from '$lib/components/ui/trace'
  import type { PlanState, PlanStatus } from '$lib/ipc/engine'

  let { plan, running = false }: { plan: PlanState; running?: boolean } = $props()

  let open = $state(true)

  const done = $derived(plan.todos.filter((step) => step.status === 'completed').length)
  const working = $derived(running && done < plan.todos.length)

  const STATUS_LABEL: Record<PlanStatus, string> = {
    get pending() { return t('conversation.toDo') },
    get in_progress() { return t('conversation.planInProgress') },
    get completed() { return t('conversation.planCompleted') },
  }
</script>

<section aria-label={t('conversation.plan')} class="rounded-lg border border-border bg-card px-3 py-2 text-card-foreground">
  <Trace.Root>
    <Trace.Collapsible bind:open>
      <Trace.CollapsibleTrigger>
        <Trace.Line status={working ? 'running' : undefined}>
          <Trace.Icon>
            <ListCheck />
          </Trace.Icon>
          <Trace.Title>{t('conversation.plan')}</Trace.Title>
          <Trace.Description aria-label={t('conversation.ofStepsDone', { done, todos: plan.todos.length })}>
            {t('conversation.stepsDone', { done, todos: plan.todos.length })}
          </Trace.Description>
        </Trace.Line>
      </Trace.CollapsibleTrigger>
      <Trace.CollapsibleContent>
        <Trace.Root>
          {#each plan.todos as step, i (i)}
            <Trace.Line
              status={step.status === 'pending'
                ? 'pending'
                : working && step.status === 'in_progress'
                  ? 'running'
                  : undefined}
            >
              <Trace.Icon>
                {#if step.status === 'completed'}
                  <CircleCheck />
                {:else if step.status === 'in_progress'}
                  <CircleDot />
                {:else}
                  <Circle />
                {/if}
              </Trace.Icon>
              <Trace.Title class="whitespace-normal break-words">
                {step.status === 'in_progress' ? step.activeForm : step.content}
                <span class="sr-only">({STATUS_LABEL[step.status]})</span>
              </Trace.Title>
            </Trace.Line>
          {/each}
        </Trace.Root>
        {#if plan.explanation}
          <p class="mt-2 text-xs break-words">{plan.explanation}</p>
        {/if}
      </Trace.CollapsibleContent>
    </Trace.Collapsible>
  </Trace.Root>
</section>
