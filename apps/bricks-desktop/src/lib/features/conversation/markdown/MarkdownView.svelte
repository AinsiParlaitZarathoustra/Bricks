<script lang="ts">
  // Rendered Markdown of one text: an assistant answer, or a sub-agent's task
  // or result. One MarkdownDocument per instance (its caches, ids and math
  // macros are its own); `idPrefix` must be unique per text and rendering zone.
  // The caller throttles `source` while it streams.
  import type { FileReference, MarkdownHeading } from './types'
  import { MarkdownDocument, markdownClicks } from '../markdown'
  import { loadKatex } from './math'
  import { createMermaidCache } from './mermaid'
  import DiffBlock from './DiffBlock.svelte'
  import MermaidBlock from './MermaidBlock.svelte'

  interface Props {
    source: string
    streaming?: boolean
    idPrefix: string
    /** Opens a file reference; absent: references and diff file names are not actionable. */
    onFile?: (ref: FileReference) => void
    /** Result of an attempt to open an external link: an error message, or null. */
    onLinkResult?: (error: string | null) => void
    /** The headings of the text, for a table of contents. */
    headings?: MarkdownHeading[]
    class?: string
  }

  let {
    source,
    streaming = false,
    idPrefix,
    onFile,
    onLinkResult,
    headings = $bindable([]),
    class: className = 'text-sm',
  }: Props = $props()

  // svelte-ignore state_referenced_locally
  const doc = new MarkdownDocument({ idPrefix })
  const mermaidCache = createMermaidCache()
  /** Bumped when the math renderer arrives, so the blocks with formulas render again. */
  let mathVersion = $state(0)

  const blocks = $derived.by(() => {
    void mathVersion
    return doc.update(source, !streaming)
  })
  $effect(() => {
    void blocks
    headings = doc.headings
  })

  // KaTeX is loaded only for a text that has a formula, once. The result is
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
        // The formulas keep their source text; a later text retries the load.
      },
    )
  })
  $effect(() => () => {
    mounted = false
    doc.dispose()
  })
</script>

<div
  class={['md leading-relaxed break-words text-foreground', className]}
  class:streaming
  use:markdownClicks={{
    onLinkResult,
    linksEnabled: !streaming,
    onFileReference: onFile,
  }}
>
  {#each blocks as block (block.key)}
    {#if block.kind === 'html'}
      {@html block.html}
    {:else if block.kind === 'mermaid'}
      <MermaidBlock source={block.source} complete={block.complete} id="{idPrefix}-{block.key}" cache={mermaidCache} />
    {:else}
      <DiffBlock source={block.source} complete={block.complete} {onFile} />
    {/if}
  {/each}
</div>

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
