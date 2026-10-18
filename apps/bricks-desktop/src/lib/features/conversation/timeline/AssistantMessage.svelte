<script lang="ts">
  import Brain from '@lucide/svelte/icons/brain'
  import ChevronDown from '@lucide/svelte/icons/chevron-down'
  import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '$lib/components/ui/collapsible'
  import type { AssistantItem, Conversation } from '$lib/state/conversation.svelte'
  import { ui } from '$lib/state/ui.svelte'
  import { throttledValue } from '../throttled.svelte'
  import { MarkdownDocument, markdownClicks } from '../markdown'
  import { loadKatex } from '../markdown/math'
  import { createMermaidCache } from '../markdown/mermaid'
  import type { FileReference } from '../markdown/types'
  import DiffBlock from '../markdown/DiffBlock.svelte'
  import MermaidBlock from '../markdown/MermaidBlock.svelte'

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

  // One document per message: its caches, ids and math macros are its own.
  // The length in front makes the prefixes of two messages never prefixes of
  // each other, so their heading and note ids cannot collide.
  // svelte-ignore state_referenced_locally
  const safeId = item.id.toLowerCase().replace(/[^a-z0-9-]/g, '-')
  const idPrefix = `m${safeId.length}-${safeId}`
  const doc = new MarkdownDocument({ idPrefix })
  const mermaidCache = createMermaidCache()
  /** Bumped when the math renderer arrives, so the blocks with formulas render again. */
  let mathVersion = $state(0)

  const blocks = $derived.by(() => {
    void mathVersion
    return doc.update(item.streaming ? throttled.value : item.text, !item.streaming)
  })
  const headings = $derived.by(() => {
    void blocks
    return doc.headings
  })
  /** A table of contents only helps a finished answer with several headings. */
  const showToc = $derived(!item.streaming && headings.length >= 3)

  // KaTeX is loaded only for a message that has a formula, once. The result is
  // kept even if more text arrived meanwhile; only unmounting drops it.
  let mathRequested = false
  let mounted = true
  $effect(() => {
    void blocks
    if (mathRequested || !doc.needsMath) return
    mathRequested = true
    loadKatex().then(
      (renderer) => {
        if (!mounted) return
        doc.setMathRenderer(renderer)
        mathVersion += 1
      },
      () => {
        // The formulas keep their source text; a later message retries the load.
      },
    )
  })
  $effect(() => () => {
    mounted = false
    doc.dispose()
  })

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
    <div
      class="md text-sm leading-relaxed break-words text-foreground"
      class:streaming={item.streaming}
      use:markdownClicks={{
        onLinkResult: (error) => (linkError = error),
        linksEnabled: !item.streaming,
        onFileReference: openFile,
      }}
    >
      {#each blocks as block (block.key)}
        {#if block.kind === 'html'}
          {@html block.html}
        {:else if block.kind === 'mermaid'}
          <MermaidBlock source={block.source} complete={block.complete} id="{idPrefix}-{block.key}" cache={mermaidCache} />
        {:else}
          <DiffBlock source={block.source} complete={block.complete} onFile={conversation?.projectId ? openFile : undefined} />
        {/if}
      {/each}
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
  .md :global([id]) {
    scroll-margin-top: 0.75rem;
  }
  /* Targets of local anchors and notes receive focus programmatically: a
     discreet mark instead of the full focus ring. */
  .md :global([tabindex='-1']:focus) {
    outline: none;
    border-radius: 0.125rem;
    background: color-mix(in oklab, var(--info-soft) 60%, transparent);
  }
  .md :global(.md-alert) {
    margin: 0.5rem 0;
    border-left: 2px solid var(--md-alert-accent);
    padding-left: 0.75rem;
  }
  .md :global(.md-alert-title) {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    margin: 0 0 0.125rem;
    font-size: 0.8125rem;
    font-weight: 600;
    color: var(--md-alert-accent);
  }
  .md :global(.md-alert-title + *) {
    margin-top: 0;
  }
  .md :global(.md-alert > :last-child) {
    margin-bottom: 0;
  }
  .md :global(.md-alert-title::before) {
    content: '';
    width: 0.875rem;
    height: 0.875rem;
    flex: none;
    background: currentColor;
    mask: var(--md-alert-icon) center / contain no-repeat;
  }
  .md :global(.md-alert-note),
  .md :global(.md-alert-important) {
    --md-alert-accent: var(--info);
    --md-alert-icon: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='black' stroke-width='2' stroke-linecap='round'%3E%3Ccircle cx='12' cy='12' r='10'/%3E%3Cpath d='M12 16v-4M12 8h.01'/%3E%3C/svg%3E");
  }
  .md :global(.md-alert-important) {
    --md-alert-accent: var(--primary);
    --md-alert-icon: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='black' stroke-width='2' stroke-linecap='round'%3E%3Cpath d='M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z'/%3E%3Cpath d='M12 7v4M12 15h.01'/%3E%3C/svg%3E");
  }
  .md :global(.md-alert-tip) {
    --md-alert-accent: var(--success);
    --md-alert-icon: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='black' stroke-width='2' stroke-linecap='round'%3E%3Cpath d='M9 18h6M10 22h4M12 2a7 7 0 0 0-4 12.7V17h8v-2.3A7 7 0 0 0 12 2z'/%3E%3C/svg%3E");
  }
  .md :global(.md-alert-warning) {
    --md-alert-accent: var(--warning);
    --md-alert-icon: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='black' stroke-width='2' stroke-linecap='round'%3E%3Cpath d='M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z'/%3E%3Cpath d='M12 9v4M12 17h.01'/%3E%3C/svg%3E");
  }
  .md :global(.md-alert-caution) {
    --md-alert-accent: var(--destructive);
    --md-alert-icon: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='black' stroke-width='2' stroke-linecap='round'%3E%3Cpath d='M7.9 2h8.2L22 7.9v8.2L16.1 22H7.9L2 16.1V7.9z'/%3E%3Cpath d='M12 8v4M12 16h.01'/%3E%3C/svg%3E");
  }
  .md :global(.fn-ref) {
    font-size: 0.75em;
    line-height: 0;
  }
  .md :global(.fn-ref a),
  .md :global(.footnotes a) {
    color: var(--primary);
    text-decoration: none;
  }
  .md :global(.footnotes) {
    margin-top: 0.75rem;
    padding-top: 0.5rem;
    border-top: 1px solid color-mix(in oklab, var(--border) 50%, transparent);
    font-size: 0.8125rem;
    color: var(--muted-foreground);
  }
  .md :global(.footnotes ol) {
    margin: 0;
  }
  .md :global(.file-ref) {
    border-radius: 0.125rem;
    color: var(--primary);
    font-family: var(--font-mono);
    font-size: 0.8125em;
    text-decoration: underline;
    text-decoration-style: dotted;
    text-underline-offset: 2px;
    cursor: pointer;
  }
  .md :global(.file-ref:hover) {
    opacity: 0.8;
  }
  .md :global(.file-ref:focus-visible) {
    outline: 2px solid var(--ring);
    outline-offset: 1px;
  }
  .md :global(.math-src),
  .md :global(.math-error) {
    font-family: var(--font-mono);
    font-size: 0.8125em;
    color: var(--muted-foreground);
    white-space: pre-wrap;
  }
  .md :global(.math-error) {
    text-decoration: underline wavy var(--destructive);
    text-underline-offset: 3px;
  }
  .md :global(.katex-display) {
    margin: 0.5rem 0;
    overflow-x: auto;
    overflow-y: hidden;
    padding: 0.125rem 0;
  }
  .md :global(.katex) {
    font-size: 1.05em;
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
