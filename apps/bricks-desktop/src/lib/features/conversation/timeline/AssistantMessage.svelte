<script lang="ts">
  import Brain from '@lucide/svelte/icons/brain'
  import ChevronDown from '@lucide/svelte/icons/chevron-down'
  import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '$lib/components/ui/collapsible'
  import type { AssistantItem } from '$lib/state/conversation.svelte'
  import { throttledValue } from '../throttled.svelte'
  import { markdownClicks, renderMarkdown } from '../markdown'

  let { item }: { item: AssistantItem } = $props()

  let reasoningOpen = $state(false)
  /** Error of the last link that could not be opened; cleared by the next successful open. */
  let linkError = $state<string | null>(null)
  // Re-render at most every 80 ms while the answer streams; the final text
  // is rendered as soon as the message completes.
  const throttled = throttledValue(
    () => item.text,
    () => item.streaming,
    80,
  )
  const html = $derived(renderMarkdown(item.streaming ? throttled.value : item.text))
  const hasReasoning = $derived(item.thinking.trim().length > 0)
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

  {#if item.text}
    <div
      class="md text-sm leading-relaxed break-words text-foreground"
      class:streaming={item.streaming}
      use:markdownClicks={{ onLinkResult: (error) => (linkError = error), linksEnabled: !item.streaming }}
    >
      {@html html}
    </div>
    {#if linkError}
      <p class="text-xs break-words text-destructive" role="alert">
        Le lien n’a pas pu être ouvert : {linkError}
      </p>
    {/if}
  {:else if item.streaming}
    <span class="inline-block h-4 w-0.5 animate-pulse rounded-sm bg-muted-foreground" aria-hidden="true"></span>
  {/if}
</article>

<style>
  .md {
    overflow-wrap: anywhere;
  }
  .md :global(p) {
    margin: 0.5rem 0;
  }
  .md :global(p:first-child),
  .md :global(h1:first-child),
  .md :global(h2:first-child),
  .md :global(h3:first-child) {
    margin-top: 0;
  }
  .md :global(p:last-child),
  .md :global(ul:last-child),
  .md :global(ol:last-child) {
    margin-bottom: 0;
  }
  .md :global(ul),
  .md :global(ol) {
    margin: 0.5rem 0;
    padding-left: 1.25rem;
  }
  .md :global(ul) {
    list-style: disc;
  }
  .md :global(ol) {
    list-style: decimal;
  }
  .md :global(li + li) {
    margin-top: 0.25rem;
  }
  .md :global(h2),
  .md :global(h3),
  .md :global(h4),
  .md :global(h5),
  .md :global(h6) {
    margin: 0.75rem 0 0.25rem;
    font-weight: 600;
    line-height: 1.3;
  }
  .md :global(h2) {
    font-size: 1.0625rem;
  }
  .md :global(h3) {
    font-size: 1rem;
  }
  .md :global(h4),
  .md :global(h5),
  .md :global(h6) {
    font-size: 0.9375rem;
  }
  .md :global(blockquote) {
    margin: 0.5rem 0;
    border-left: 2px solid var(--border);
    padding-left: 0.75rem;
    color: var(--muted-foreground);
  }
  .md :global(hr) {
    margin: 0.75rem 0;
    border-color: var(--border);
  }
  .md :global(table) {
    display: block;
    max-width: 100%;
    overflow-x: auto;
    border-collapse: collapse;
    font-size: 0.8125rem;
  }
  .md :global(th),
  .md :global(td) {
    border: 1px solid var(--border);
    padding: 0.25rem 0.5rem;
    text-align: left;
    vertical-align: top;
  }
  .md :global(:not(pre) > code) {
    border-radius: 0.25rem;
    background: var(--code-bg);
    color: var(--code-foreground);
    padding: 0.05rem 0.3rem;
    font-family: var(--font-mono);
    font-size: 0.8125em;
  }
  .md :global(.hljs-keyword),
  .md :global(.hljs-selector-tag),
  .md :global(.hljs-built_in),
  .md :global(.hljs-type),
  .md :global(.hljs-title.class_) {
    color: var(--primary);
  }
  .md :global(.hljs-string),
  .md :global(.hljs-addition) {
    color: var(--success);
  }
  .md :global(.hljs-addition) {
    color: var(--diff-add);
  }
  .md :global(.hljs-deletion) {
    color: var(--diff-del);
  }
  .md :global(.hljs-number),
  .md :global(.hljs-literal),
  .md :global(.hljs-attr),
  .md :global(.hljs-variable),
  .md :global(.hljs-template-variable) {
    color: var(--info);
  }
  .md :global(.hljs-comment),
  .md :global(.hljs-quote),
  .md :global(.hljs-meta) {
    color: var(--muted-foreground);
    font-style: italic;
  }
  .md.streaming > :global(:last-child)::after {
    content: '';
    display: inline-block;
    width: 0.125rem;
    height: 1em;
    margin-left: 0.125rem;
    vertical-align: text-bottom;
    background: var(--muted-foreground);
    animation: caret 1s steps(2, start) infinite;
  }
  @keyframes caret {
    to {
      visibility: hidden;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .md.streaming > :global(:last-child)::after {
      animation: none;
    }
  }
</style>
