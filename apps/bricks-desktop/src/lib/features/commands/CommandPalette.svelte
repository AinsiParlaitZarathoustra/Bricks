<script lang="ts">
  // The `/` palette above the composer: an ARIA listbox. The keyboard stays in
  // the composer's editor (combobox pattern: aria-activedescendant), so this
  // component only renders the options and reports clicks and hovers.
  // Owner: Haiku 3 (Sprint 14.1).
  import type { SlashCommandInfo } from '$lib/ipc/bindings/SlashCommandInfo'
  import type { SlashRegistryState } from './slash-registry.svelte'
  import { usageLine } from './palette'

  let {
    id,
    items,
    activeIndex,
    status,
    error = '',
    query,
    onPick,
    onHover,
  }: {
    /** Base id: the listbox is `id`, its options are `${id}-opt-${index}`. */
    id: string
    items: SlashCommandInfo[]
    activeIndex: number
    status: SlashRegistryState['status']
    error?: string
    query: string
    onPick: (cmd: SlashCommandInfo) => void
    onHover: (index: number) => void
  } = $props()

  // Keeps the highlighted option visible while the arrow keys move it.
  $effect(() => {
    const index = activeIndex
    void items
    document.getElementById(`${id}-opt-${index}`)?.scrollIntoView({ block: 'nearest' })
  })
</script>

<div
  class="absolute inset-x-0 bottom-full z-20 mb-2 overflow-hidden rounded-lg border bg-popover text-popover-foreground shadow-md"
>
  {#if status === 'loading'}
    <p role="status" class="px-3 py-2 text-sm text-muted-foreground">Chargement des commandes…</p>
  {:else if status === 'error'}
    <p role="alert" class="px-3 py-2 text-sm text-destructive">Commandes indisponibles : {error}</p>
  {:else if items.length === 0}
    <p role="status" class="px-3 py-2 text-sm text-muted-foreground">
      Aucune commande ne correspond à « /{query} ».
    </p>
  {:else}
    <div role="listbox" {id} aria-label="Commandes" class="max-h-64 overflow-y-auto p-1">
      {#each items as cmd, i (cmd.name)}
        <div
          role="option"
          id="{id}-opt-{i}"
          aria-selected={i === activeIndex}
          tabindex="-1"
          class="flex cursor-pointer flex-col gap-0.5 rounded-md px-2 py-1.5 text-sm {i === activeIndex
            ? 'bg-accent text-accent-foreground'
            : ''}"
          onmousedown={(e) => e.preventDefault()}
          onmouseenter={() => onHover(i)}
          onclick={() => onPick(cmd)}
          onkeydown={(e) => {
            // Options are not tab stops (the editor keeps the focus); Enter or Space still pick one.
            if (e.key !== 'Enter' && e.key !== ' ') return
            e.preventDefault()
            onPick(cmd)
          }}
        >
          <span class="flex flex-wrap items-baseline gap-x-2">
            <span class="font-mono font-medium">{usageLine(cmd)}</span>
            {#if cmd.aliases.length > 0}
              <span class="text-xs text-muted-foreground">
                alias : {cmd.aliases.map((a) => `/${a}`).join(', ')}
              </span>
            {/if}
          </span>
          <span class="text-xs text-muted-foreground">{cmd.description}</span>
        </div>
      {/each}
    </div>
  {/if}
</div>
