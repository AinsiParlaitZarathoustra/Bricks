<script lang="ts">
  import Brain from '@lucide/svelte/icons/brain'
  import Check from '@lucide/svelte/icons/check'
  import ChevronDown from '@lucide/svelte/icons/chevron-down'
  import Copy from '@lucide/svelte/icons/copy'
  import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '$lib/components/ui/collapsible'
  import type { AssistantItem, Conversation } from '$lib/state/conversation.svelte'
  import { ui } from '$lib/state/ui.svelte'
  import { throttledValue } from '../throttled.svelte'
  import type { FileReference, MarkdownHeading } from '../markdown/types'
  import MarkdownView from '../markdown/MarkdownView.svelte'

  let { item, conversation }: { item: AssistantItem; conversation?: Conversation } = $props()

  let reasoningOpen = $state(false)
  let tocOpen = $state(false)
  /** Error of the last link or file reference that could not be opened; cleared by the next successful open. */
  let linkError = $state<string | null>(null)
  // The source is followed at most every 80 ms while the answer streams; the
  // final text is reconciled as soon as the message completes.
  const throttled = throttledValue(
    () => item.text,
    () => item.streaming,
    80,
  )

  // The length in front makes the prefixes of two messages never prefixes of
  // each other, so their heading and note ids cannot collide.
  // svelte-ignore state_referenced_locally
  const safeId = item.id.toLowerCase().replace(/[^a-z0-9-]/g, '-')
  const idPrefix = `m${safeId.length}-${safeId}`
  let headings = $state<MarkdownHeading[]>([])
  /** A table of contents only helps a finished answer with several headings. */
  const showToc = $derived(!item.streaming && headings.length >= 3)

  const hasReasoning = $derived(item.thinking.trim().length > 0)

  function openFile(ref: FileReference) {
    const projectId = conversation?.projectId
    if (!projectId) {
      linkError = 'cette conversation n’est rattachée à aucun projet.'
      return
    }
    linkError = null
    ui.open('file', {
      projectId,
      path: ref.path,
      line: ref.line !== undefined ? String(ref.line) : undefined,
      endLine: ref.endLine !== undefined ? String(ref.endLine) : undefined,
    })
  }

  // Copy of this answer's Markdown source only (no reasoning, tools or run summary).
  let copied = $state(false)
  let copyError = $state<string | null>(null)
  let copiedTimer: ReturnType<typeof setTimeout> | undefined
  $effect(() => () => clearTimeout(copiedTimer))

  async function copyAnswer() {
    try {
      await navigator.clipboard.writeText(item.text)
      copyError = null
      copied = true
      clearTimeout(copiedTimer)
      copiedTimer = setTimeout(() => (copied = false), 1500)
    } catch (e) {
      copied = false
      copyError = e instanceof Error ? e.message : String(e)
    }
  }

  function jumpTo(id: string) {
    const target = document.getElementById(id)
    if (!target) return
    if (!target.hasAttribute('tabindex')) target.setAttribute('tabindex', '-1')
    target.scrollIntoView({ block: 'start' })
    target.focus({ preventScroll: true })
  }
</script>

<article class="flex min-w-0 flex-col gap-1.5" aria-busy={item.streaming}>
  {#if hasReasoning}
    <Collapsible bind:open={reasoningOpen}>
      <CollapsibleTrigger
        class="group inline-flex w-fit items-center gap-1.5 rounded-md px-1.5 py-1 text-xs text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
      >
        <Brain class="size-3.5" aria-hidden="true" />
        Raisonnement exposé par le fournisseur
        <ChevronDown class="size-3.5 transition-transform group-data-[state=open]:rotate-180" aria-hidden="true" />
      </CollapsibleTrigger>
      <CollapsibleContent>
        <p class="mt-1 border-l-2 border-border pl-3 text-xs break-words whitespace-pre-wrap text-muted-foreground">
          {item.thinking}
        </p>
      </CollapsibleContent>
    </Collapsible>
  {/if}

  {#if showToc}
    <Collapsible bind:open={tocOpen}>
      <CollapsibleTrigger
        class="group inline-flex w-fit items-center gap-1.5 rounded-md px-1.5 py-1 text-xs text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
      >
        Sommaire
        <ChevronDown class="size-3.5 transition-transform group-data-[state=open]:rotate-180" aria-hidden="true" />
      </CollapsibleTrigger>
      <CollapsibleContent>
        <nav aria-label="Sommaire de la réponse" class="mt-1 text-xs">
          <ul class="flex flex-col gap-0.5">
            {#each headings as heading (heading.id)}
              <li style:padding-left="{(heading.level - 1) * 0.75}rem">
                <button
                  type="button"
                  class="rounded-sm text-left text-muted-foreground hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                  onclick={() => jumpTo(heading.id)}
                >
                  {heading.text}
                </button>
              </li>
            {/each}
          </ul>
        </nav>
      </CollapsibleContent>
    </Collapsible>
  {/if}

  {#if item.text}
    <MarkdownView
      source={item.streaming ? throttled.value : item.text}
      streaming={item.streaming}
      {idPrefix}
      onFile={openFile}
      onLinkResult={(error) => (linkError = error)}
      bind:headings
    />
    {#if linkError}
      <p class="text-xs break-words text-destructive" role="alert">
        Le lien n’a pas pu être ouvert : {linkError}
      </p>
    {/if}
    {#if !item.streaming}
      <div class="mt-1 flex items-center gap-2">
        <button
          type="button"
          class="inline-flex size-6 items-center justify-center rounded-md text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
          title="Copier"
          aria-label="Copier"
          onclick={copyAnswer}
        >
          {#if copied}
            <Check class="size-3.5" aria-hidden="true" />
          {:else}
            <Copy class="size-3.5" aria-hidden="true" />
          {/if}
        </button>
        {#if copyError}
          <p class="text-xs text-destructive" role="alert">Copie impossible : {copyError}</p>
        {/if}
      </div>
    {/if}
  {:else if item.streaming}
    <span class="inline-block h-4 w-0.5 animate-pulse rounded-sm bg-muted-foreground" aria-hidden="true"></span>
  {/if}
</article>
