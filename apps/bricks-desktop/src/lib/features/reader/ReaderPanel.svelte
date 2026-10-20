<script lang="ts">
  // Read-only reader of the right column: one tab per file of the open
  // conversation, and per tab a source view (highlighted, numbered lines,
  // read in bounded windows), a Markdown preview, and the recorded edit
  // operations of that file (each patch on its own, never merged).
  import X from '@lucide/svelte/icons/x'
  import DiffView from '$lib/features/changes/DiffView.svelte'
  import SvgViewer from '$lib/features/conversation/markdown/SvgViewer.svelte'
  import MarkdownView from '$lib/features/conversation/markdown/MarkdownView.svelte'
  import DiffStat from '$lib/layout/DiffStat.svelte'
  import type { Conversation } from '$lib/state/conversation.svelte'
  import { reader, type ReaderTab } from '$lib/state/reader.svelte'
  import { highlightLines } from './highlight'

  let { conversation }: { conversation: Conversation | null } = $props()

  const sessionId = $derived(conversation?.sessionId ?? null)
  const tabs = $derived(sessionId ? reader.tabs(sessionId) : [])
  const active = $derived(sessionId ? reader.active(sessionId) : null)

  const highlighted = $derived.by(() => {
    const w = active?.window
    if (!active || !w || w.lines.length === 0) return []
    return highlightLines(w.lines.join('\n'), active.language)
  })

  const MODES: { id: ReaderTab['mode']; label: string }[] = [
    { id: 'source', label: 'Source' },
    { id: 'preview', label: 'Aperçu' },
    { id: 'changes', label: 'Modifications' },
  ]
  function available(tab: ReaderTab, mode: ReaderTab['mode']): boolean {
    if (mode === 'preview') return tab.isMarkdown || tab.isSvg
    if (mode === 'changes') return tab.operations.length > 0
    return true
  }

  // Each tab keeps its scroll position when the view changes.
  let scroller = $state<HTMLDivElement | null>(null)
  $effect(() => {
    const el = scroller
    const tab = active
    if (!el || !tab) return
    el.scrollTop = tab.scrollTop
  })
  function onScroll() {
    if (active && scroller) reader.rememberScroll(active.id, scroller.scrollTop)
  }

  function prefixOf(tab: ReaderTab): string {
    const safe = tab.id.toLowerCase().replace(/[^a-z0-9-]/g, '-')
    return `r${safe.length}-${safe}`
  }

  const nf = new Intl.NumberFormat('fr-FR')
</script>

<section aria-label="Lecteur" class="flex h-full min-h-0 flex-col bg-background">
  {#if tabs.length === 0}
    <p class="p-4 text-[13px] text-muted-foreground">Aucun fichier ouvert.</p>
  {:else}
    <div role="tablist" aria-label="Fichiers ouverts" class="flex shrink-0 items-center gap-0.5 overflow-x-auto border-b border-border/50 px-1.5 py-1">
      {#each tabs as tab (tab.id)}
        <div
          class={[
            'group flex shrink-0 items-center rounded-md text-[13px]',
            tab.id === active?.id ? 'bg-muted text-foreground' : 'text-muted-foreground hover:bg-muted/60',
          ]}
        >
          <button
            type="button"
            role="tab"
            aria-selected={tab.id === active?.id}
            title={tab.fullPath}
            class="max-w-48 truncate rounded-md py-1 pr-1 pl-2 font-mono text-xs focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
            onclick={() => reader.activate(tab.id)}
          >
            {tab.title}
          </button>
          <button
            type="button"
            aria-label={`Fermer ${tab.title}`}
            title="Fermer"
            class="mr-0.5 inline-flex size-5 items-center justify-center rounded text-muted-foreground hover:bg-background hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
            onclick={() => reader.close(tab.id)}
          >
            <X class="size-3" aria-hidden="true" />
          </button>
        </div>
      {/each}
    </div>

    {#if active}
      <div class="flex shrink-0 items-center gap-2 px-2 py-1">
        <div role="radiogroup" aria-label="Mode de lecture" class="flex items-center gap-0.5">
          {#each MODES as mode (mode.id)}
            {#if available(active, mode.id)}
              <button
                type="button"
                role="radio"
                aria-checked={active.mode === mode.id}
                class={[
                  'rounded-md px-2 py-0.5 text-xs focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none',
                  active.mode === mode.id ? 'bg-muted text-foreground' : 'text-muted-foreground hover:text-foreground',
                ]}
                onclick={() => reader.setMode(active.id, mode.id)}
              >
                {mode.label}
              </button>
            {/if}
          {/each}
        </div>
        <span class="ml-auto min-w-0 truncate font-mono text-[11px] text-muted-foreground" title={active.fullPath}>
          {active.fullPath}
        </span>
      </div>

      <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
      <div
        bind:this={scroller}
        onscroll={onScroll}
        role="region"
        aria-label={`Contenu de ${active.title}`}
        tabindex="0"
        class="min-h-0 flex-1 overflow-auto outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset"
      >
        {#if active.mode === 'changes'}
          <div class="flex flex-col gap-3 p-2">
            {#each active.operations as op, i (op.key)}
              <section aria-label={`Opération ${i + 1}`} class="flex flex-col gap-1">
                <p class="flex items-center gap-2 px-0.5 text-xs text-muted-foreground">
                  <span>Opération {i + 1} sur {active.operations.length}</span>
                  <DiffStat additions={op.added} deletions={op.removed} />
                </p>
                {#if op.patch}
                  <DiffView patch={op.patch} truncated={op.truncated} loose />
                {:else if op.truncated}
                  <p class="px-0.5 text-xs text-muted-foreground">Modifications trop volumineuses : non conservées pour cette opération.</p>
                {:else}
                  <p class="px-0.5 text-xs text-muted-foreground">Modifications non enregistrées pour cette opération.</p>
                {/if}
              </section>
            {/each}
          </div>
        {:else if !active.window || active.window.loading}
          <p class="p-3 text-xs text-muted-foreground" role="status">Lecture…</p>
        {:else if active.window.error}
          <p class="p-3 text-xs text-destructive" role="alert">{active.window.error}</p>
        {:else if active.mode === 'preview'}
          <div class="flex flex-col gap-2 p-3">
            {#if active.window.truncated}
              <p class="text-xs text-muted-foreground">
                Aperçu partiel : lignes {nf.format(active.window.start)} à {nf.format(active.window.start + active.window.lines.length - 1)}
                sur {nf.format(active.window.totalLines)}.
              </p>
            {/if}
            {#key active.id}
              {#if active.isSvg}
                {#if active.window.truncated}
                  <p class="text-xs text-muted-foreground">Le fichier SVG doit être entièrement chargé pour afficher son aperçu.</p>
                {:else}
                  <SvgViewer svg={active.window.lines.join('\n')} title={active.title} fileName={active.title} />
                {/if}
              {:else}
                <MarkdownView source={active.window.lines.join('\n')} idPrefix={prefixOf(active)} />
              {/if}
            {/key}
          </div>
        {:else}
          <div class="min-w-max py-1 font-mono text-xs leading-5">
            {#each highlighted as html, i (i)}
              <div class="grid grid-cols-[3.5rem_minmax(0,1fr)]">
                <span class="px-2 text-right text-muted-foreground tabular-nums select-none">{active.window.start + i}</span>
                <span class="hljs pr-3 whitespace-pre">{@html html || ' '}</span>
              </div>
            {/each}
          </div>
        {/if}
      </div>

      {#if active.mode !== 'changes' && active.window && !active.window.loading && !active.window.error && active.window.truncated}
        {@const w = active.window}
        <div class="flex shrink-0 items-center gap-2 border-t border-border/50 px-2 py-1 text-xs text-muted-foreground">
          <span class="tabular-nums">
            Lignes {nf.format(w.start)}–{nf.format(w.start + w.lines.length - 1)} sur {nf.format(w.totalLines)}
          </span>
          <button
            type="button"
            class="ml-auto rounded px-1.5 py-0.5 hover:bg-muted hover:text-foreground disabled:opacity-40"
            disabled={w.start <= 1}
            onclick={() => reader.loadWindow(active.id, Math.max(1, w.start - 400))}
          >
            Précédentes
          </button>
          <button
            type="button"
            class="rounded px-1.5 py-0.5 hover:bg-muted hover:text-foreground disabled:opacity-40"
            disabled={w.start + w.lines.length > w.totalLines}
            onclick={() => reader.loadWindow(active.id, w.start + w.lines.length)}
          >
            Suivantes
          </button>
        </div>
      {/if}
    {/if}
  {/if}
</section>

<style>
  .hljs :global(.hljs-keyword),
  .hljs :global(.hljs-selector-tag),
  .hljs :global(.hljs-built_in),
  .hljs :global(.hljs-type),
  .hljs :global(.hljs-title.class_) {
    color: var(--primary);
  }
  .hljs :global(.hljs-string) {
    color: var(--success);
  }
  .hljs :global(.hljs-number),
  .hljs :global(.hljs-literal),
  .hljs :global(.hljs-attr),
  .hljs :global(.hljs-variable) {
    color: var(--info);
  }
  .hljs :global(.hljs-comment),
  .hljs :global(.hljs-quote),
  .hljs :global(.hljs-meta) {
    color: var(--muted-foreground);
    font-style: italic;
  }
</style>
