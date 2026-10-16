<script lang="ts">
  import FilePen from '@lucide/svelte/icons/file-pen'
  import DiffStat from '$lib/layout/DiffStat.svelte'
  import type { Conversation, EditItem } from '$lib/state/conversation.svelte'
  import { CHANGE_KIND_LABEL } from '../format'

  let { item, conversation }: { item: EditItem; conversation: Conversation } = $props()

  const agentName = $derived.by(() => {
    if (!item.agentId) return null
    return conversation.agents.get(item.agentId)?.spawn.profile ?? `agent inconnu (${item.agentId.slice(0, 8)})`
  })
</script>

<section aria-label="Fichiers modifiés" class="rounded-md border border-border bg-card px-2.5 py-2 text-xs text-card-foreground">
  <header class="flex flex-wrap items-center gap-x-2 gap-y-1 text-muted-foreground">
    <FilePen class="size-3.5 shrink-0" aria-hidden="true" />
    <h2 class="font-medium text-foreground">Fichiers modifiés</h2>
    <span class="font-mono">{item.tool}</span>
    {#if agentName}
      <span>Agent : {agentName}</span>
    {/if}
  </header>
  <ul class="mt-1.5 flex flex-col gap-1">
    {#each item.files as file, i (i)}
      <li class="flex flex-wrap items-center gap-x-2 gap-y-1">
        <span class="text-muted-foreground">{CHANGE_KIND_LABEL[file.kind]}</span>
        <span class="min-w-0 break-all font-mono">{file.path}</span>
        {#if file.binary}
          <span class="text-muted-foreground">binaire</span>
        {:else}
          <DiffStat additions={file.added} deletions={file.removed} label="lignes de l’opération" />
        {/if}
      </li>
    {/each}
  </ul>
</section>
