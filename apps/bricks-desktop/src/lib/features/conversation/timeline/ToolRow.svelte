<script lang="ts">
  import { ChevronDown, CircleCheck, CircleX, LoaderCircle, Terminal, Wrench } from '@lucide/svelte'
  import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '$lib/components/ui/collapsible'
  import type { ToolItem } from '$lib/state/conversation.svelte'
  import { SHELL_TOOLS, TOOL_STATUS_LABEL, formatDuration, prettyJson, summarizeInput } from '../format'
  import MonoBlock from './MonoBlock.svelte'

  let { item }: { item: ToolItem } = $props()

  let open = $state(false)
  const isShell = $derived(SHELL_TOOLS.has(item.name))
  const summary = $derived(summarizeInput(item.name, item.input))
  const inputText = $derived(prettyJson(item.input))
</script>

<div class="rounded-md border border-border bg-card text-xs text-card-foreground">
  <Collapsible bind:open>
    <CollapsibleTrigger
      class="group flex w-full min-w-0 items-center gap-2 rounded-md px-2.5 py-1.5 text-left hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
    >
      <ChevronDown class="size-3.5 shrink-0 text-muted-foreground transition-transform group-data-[state=open]:rotate-180" aria-hidden="true" />
      {#if isShell}
        <Terminal class="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
      {:else}
        <Wrench class="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
      {/if}
      <span class="shrink-0 font-mono font-medium text-foreground">{item.name}</span>
      {#if summary}
        <span class="min-w-0 flex-1 truncate font-mono text-muted-foreground">{summary}</span>
      {:else}
        <span class="flex-1"></span>
      {/if}
      {#if item.status === 'running'}
        <LoaderCircle class="size-3.5 shrink-0 animate-spin text-info" aria-hidden="true" />
      {:else if item.status === 'ok'}
        <CircleCheck class="size-3.5 shrink-0 text-success" aria-hidden="true" />
      {:else}
        <CircleX class="size-3.5 shrink-0 text-destructive" aria-hidden="true" />
      {/if}
      <span class="sr-only">{TOOL_STATUS_LABEL[item.status]}</span>
      {#if item.durationMs !== undefined}
        <span class="shrink-0 whitespace-nowrap text-muted-foreground tabular-nums">{formatDuration(item.durationMs)}</span>
      {/if}
    </CollapsibleTrigger>
    {#if item.status === 'running' && item.progress}
      <p class="truncate px-2.5 pb-1.5 pl-8 text-muted-foreground">{item.progress}</p>
    {/if}
    <CollapsibleContent>
      <div class="flex flex-col gap-2 border-t border-border px-2.5 py-2">
        <div>
          <p class="mb-1 text-muted-foreground">Entrée</p>
          <MonoBlock text={inputText} label={`Entrée de ${item.name}`} />
        </div>
        {#if item.output !== undefined}
          <div>
            <p class="mb-1 text-muted-foreground">Sortie</p>
            <MonoBlock text={item.output || '(aucune sortie)'} label={`Sortie de ${item.name}`} />
          </div>
        {/if}
      </div>
    </CollapsibleContent>
  </Collapsible>
</div>
