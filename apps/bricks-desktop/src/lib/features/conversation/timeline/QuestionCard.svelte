<script lang="ts">
  // A question of `Ask`: its choices, « Autre » for a free answer, « Passer » and
  // « Envoyer » (⌘↩). With a timer, the time left comes from the engine's deadline,
  // so a reload or a switch of conversation never restarts it; the engine closes
  // the question at expiry. A closed question shows nothing here: its `Ask` line
  // counts the answers.
  import CircleAlert from '@lucide/svelte/icons/circle-alert'
  import { Button } from '$lib/components/ui/button'
  import { Input } from '$lib/components/ui/input'
  import type { QuestionAnswer, QuestionRequest } from '$lib/ipc/engine'
  import type { Conversation } from '$lib/state/conversation.svelte'
  import { app } from '$lib/state/app.svelte'
  import { createClock } from '$lib/features/navigation/clock.svelte'
  import { cn } from '$lib/utils'

  let { question, conversation }: { question: QuestionRequest; conversation: Conversation } = $props()

  const uid = $props.id()
  /** « Autre »: the index after the options. */
  const OTHER = $derived(question.options.length)

  /** The choice made, the recommended option until the person picks one. */
  let selected = $derived<number | null>(question.recommended ?? null)
  let other = $state('')
  let sending = $state(false)
  let refusal = $state<string | null>(null)

  const clock = createClock(1000)
  /** Whole seconds left, `null` without a timer. */
  const left = $derived(
    question.deadline_at === undefined
      ? null
      : Math.max(0, Math.ceil((question.deadline_at - clock.now) / 1000)),
  )
  const leftText = $derived(
    left === null ? '' : `${Math.floor(left / 60)}:${String(left % 60).padStart(2, '0')}`,
  )

  const canSend = $derived(
    !sending && selected !== null && (selected !== OTHER || other.trim() !== ''),
  )

  async function answer(a: QuestionAnswer) {
    if (sending) return
    sending = true
    refusal = await app.answerQuestion(conversation, question.question_id, a)
    sending = false
  }

  function send() {
    if (!canSend || selected === null) return
    void answer(selected === OTHER ? { kind: 'other', text: other.trim() } : { kind: 'choice', choice: selected })
  }

  function onKeydown(event: KeyboardEvent) {
    if (event.key === 'Enter' && (event.metaKey || event.ctrlKey) && !event.isComposing) {
      event.preventDefault()
      send()
    }
  }

</script>

  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <section
    aria-labelledby={`${uid}-q`}
    class="overflow-hidden rounded-lg border border-border bg-card text-card-foreground"
    onkeydown={onKeydown}
  >
    <header class="flex flex-col gap-1.5 border-b border-border bg-muted/40 px-4 py-3">
      <div class="flex items-center gap-1.5 text-caption font-medium text-warning">
        <CircleAlert class="size-3.5 shrink-0" aria-hidden="true" />
        <span>Requiert votre intervention</span>
        {#if left !== null}
          <span
            class={cn(
              'ml-auto font-mono tabular-nums',
              left < 10 ? 'text-destructive' : left < 30 ? 'text-warning' : 'text-muted-foreground',
            )}
            aria-label={`Temps restant : ${leftText}`}
          >
            {leftText}
          </span>
        {/if}
      </div>
      <h2 id={`${uid}-q`} class="text-ui font-medium break-words">{question.question}</h2>
    </header>

    <fieldset class="m-0 border-0 p-0">
      <legend class="sr-only">{question.question}</legend>
      {#each [...question.options, { label: 'Autre', description: undefined }] as option, index (index)}
        <label
          class={cn(
            'flex cursor-pointer gap-3 px-4 py-3 transition-colors hover:bg-muted/50',
            index > 0 && 'border-t border-border',
          )}
        >
          <input
            type="radio"
            name={`${uid}-choice`}
            value={index}
            class="mt-0.5 size-4 shrink-0 accent-primary"
            checked={selected === index}
            onchange={() => (selected = index)}
          />
          <span class="flex min-w-0 flex-col gap-0.5">
            <span class="text-ui break-words">
              {option.label}{#if index === question.recommended}<span class="ml-1 text-muted-foreground">(Recommandé)</span>{/if}
            </span>
            {#if option.description}
              <span class="text-caption break-words text-muted-foreground">{option.description}</span>
            {/if}
          </span>
        </label>
      {/each}
      {#if selected === OTHER}
        <div class="px-4 pb-3 pl-11">
          <Input bind:value={other} aria-label="Votre réponse" class="h-8 text-ui" />
        </div>
      {/if}
    </fieldset>

    <footer class="flex flex-wrap items-center justify-end gap-2 border-t border-border px-4 py-3">
      {#if refusal}
        <p class="mr-auto text-caption break-words text-destructive" role="alert">{refusal}</p>
      {/if}
      <Button size="sm" variant="secondary" disabled={sending} onclick={() => answer({ kind: 'skip' })}>
        Passer
      </Button>
      <Button size="sm" disabled={!canSend} onclick={send}>Envoyer</Button>
    </footer>
  </section>
