<script lang="ts">
  // The `@` popover above the composer: project files and folders matching the
  // text after `@`. Same listbox pattern as the command palette: the editor keeps
  // the keyboard (aria-activedescendant); this component renders and reports.
  import type { ProjectFileEntry } from '$lib/ipc/bindings/ProjectFileEntry'
  import { Kbd } from '$lib/components/ui/kbd'
  import FileIcon from '$lib/features/file-icons/FileIcon.svelte'
  import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw'
  import type { MentionStatus } from './mention-files.svelte'

  let {
    id,
    items,
    activeIndex,
    status,
    error = '',
    query,
    onPick,
    onHover,
    onRefresh,
  }: {
    /** Base id: the listbox is `id`, its options are `${id}-opt-${index}`. */
    id: string
    items: ProjectFileEntry[]
    activeIndex: number
    status: MentionStatus
    error?: string
    query: string
    onPick: (entry: ProjectFileEntry) => void
    onHover: (index: number) => void
    onRefresh: () => void
  } = $props()

  // Keeps the highlighted entry visible while the arrow keys move it.
  $effect(() => {
    const index = activeIndex
    void items
    document.getElementById(`${id}-opt-${index}`)?.scrollIntoView({ block: 'nearest' })
  })
</script>

<div
  class="absolute inset-x-0 bottom-full z-20 mb-2 overflow-hidden rounded-lg border bg-popover text-popover-foreground shadow-md"
>
  {#if items.length > 0}
    <div role="listbox" {id} aria-label="Fichiers du projet" class="max-h-64 overflow-y-auto p-1">
      {#each items as entry, i (entry.path)}
        <div
          role="option"
          id="{id}-opt-{i}"
          aria-selected={i === activeIndex}
          tabindex="-1"
          class="flex cursor-pointer items-center gap-1.5 rounded-md px-2 py-1.5 text-sm {i === activeIndex
            ? 'bg-accent text-accent-foreground'
            : ''}"
          onmousedown={(e) => e.preventDefault()}
          onmouseenter={() => onHover(i)}
          onclick={() => onPick(entry)}
          onkeydown={(e) => {
            // Options are not tab stops (the editor keeps the focus); Enter or Space still pick one.
            if (e.key !== 'Enter' && e.key !== ' ') return
            e.preventDefault()
            onPick(entry)
          }}
        >
          <FileIcon path={entry.path} kind={entry.is_dir ? 'folder' : 'file'} />
          <span class="truncate font-mono">{entry.path}{entry.is_dir ? '/' : ''}</span>
        </div>
      {/each}
    </div>
  {:else if status === 'error'}
    <p role="alert" class="px-3 py-2 text-sm text-destructive">Fichiers indisponibles : {error}</p>
  {:else if status === 'ready'}
    <p role="status" class="px-3 py-2 text-sm text-muted-foreground">
      Aucun fichier ne correspond à « @{query} ».
    </p>
  {:else}
    <p role="status" class="px-3 py-2 text-sm text-muted-foreground">Recherche des fichiers…</p>
  {/if}

  <div class="flex items-center justify-end gap-2 border-t px-2 py-1.5 text-xs text-muted-foreground">
    <button
      type="button"
      class="inline-flex items-center gap-1.5 rounded-sm px-1.5 py-0.5 hover:bg-accent hover:text-accent-foreground"
      aria-keyshortcuts="Control+R"
      title="Actualiser la liste des fichiers (Ctrl+R)"
      onmousedown={(e) => e.preventDefault()}
      onclick={onRefresh}
    >
      <RefreshCwIcon class="size-3.5" aria-hidden="true" />
      Actualiser
      <Kbd>Ctrl R</Kbd>
    </button>
  </div>
</div>
