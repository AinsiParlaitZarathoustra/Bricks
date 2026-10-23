<script lang="ts">
  import ChevronRight from '@lucide/svelte/icons/chevron-right'
  import Circle from '@lucide/svelte/icons/circle'
  import CircleCheck from '@lucide/svelte/icons/circle-check'
  import CircleDot from '@lucide/svelte/icons/circle-dot'
  import type { PlanState, PlanStatus } from '$lib/ipc/engine'

  let { plan }: { plan: PlanState } = $props()

  const uid = $props.id()
  let open = $state(false)

  let done = $derived(plan.todos.filter((t) => t.status === 'completed').length)
  let current = $derived(plan.todos.find((t) => t.status === 'in_progress'))

  const STATUS_LABEL: Record<PlanStatus, string> = {
    pending: 'à faire',
    in_progress: 'en cours',
    completed: 'terminée',
  }
</script>

<section aria-label="Plan" class="rounded-lg border border-border bg-card text-sm text-card-foreground">
  <button
    type="button"
    class="flex w-full min-w-0 items-center gap-2 rounded-lg px-3 py-1.5 text-left hover:bg-muted/50 focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
    aria-expanded={open}
    aria-controls="{uid}-steps"
    onclick={() => (open = !open)}
  >
    <ChevronRight
      class="size-3.5 shrink-0 text-muted-foreground transition-transform {open ? 'rotate-90' : ''}"
      aria-hidden="true"
    />
    <span class="font-medium">Plan</span>
    <span class="text-xs text-muted-foreground tabular-nums" aria-label="{done} étapes terminées sur {plan.todos.length}">
      {done}/{plan.todos.length}
    </span>
    {#if current && !open}
      <span class="min-w-0 truncate text-xs text-muted-foreground">· {current.activeForm}</span>
    {/if}
  </button>
  {#if open}
    <div id="{uid}-steps" class="flex flex-col gap-1.5 border-t border-border px-3 py-2">
      <ol class="flex flex-col gap-1">
        {#each plan.todos as step, i (i)}
          <li class="flex min-w-0 items-start gap-2 text-xs">
            {#if step.status === 'completed'}
              <CircleCheck class="mt-px size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
            {:else if step.status === 'in_progress'}
              <CircleDot class="mt-px size-3.5 shrink-0 text-primary" aria-hidden="true" />
            {:else}
              <Circle class="mt-px size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
            {/if}
            <span
              class={[
                'min-w-0 break-words',
                step.status === 'completed' && 'text-muted-foreground line-through',
                step.status === 'in_progress' && 'font-medium',
              ]}
            >
              {step.status === 'in_progress' ? step.activeForm : step.content}
              <span class="sr-only">({STATUS_LABEL[step.status]})</span>
            </span>
          </li>
        {/each}
      </ol>
      {#if plan.explanation}
        <p class="text-xs text-muted-foreground break-words">{plan.explanation}</p>
      {/if}
    </div>
  {/if}
</section>
