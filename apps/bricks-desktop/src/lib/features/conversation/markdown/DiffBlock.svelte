<script lang="ts">
  import FileIcon from '$lib/features/file-icons/FileIcon.svelte'
  // A top-level ```diff / ```patch fence: a proposal written in the answer,
  // shown read-only with the change views' DiffView. Its counts are the
  // proposal's own, never the conversation's Git statistics. Nothing here
  // applies, commits or runs the patch.
  import ChevronRight from '@lucide/svelte/icons/chevron-right'
  import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '$lib/components/ui/collapsible'
  import DiffView from '$lib/features/changes/DiffView.svelte'
  import { patchFiles } from '$lib/features/changes/patch'
  import type { FileReference } from './types'

  interface Props {
    source: string
    complete: boolean
    /** Opens a file of the conversation's project; absent when the conversation has none. */
    onFile?: (ref: FileReference) => void
  }

  let { source, complete, onFile }: Props = $props()

  const files = $derived(patchFiles(source))
  let copied = $state<string | null>(null)
  let timer: ReturnType<typeof setTimeout> | undefined
  $effect(() => () => clearTimeout(timer))

  async function copyPatch() {
    try {
      await navigator.clipboard.writeText(source)
      copied = 'Copié'
    } catch {
      copied = 'Copie impossible'
    }
    clearTimeout(timer)
    timer = setTimeout(() => (copied = null), 1500)
  }
</script>

<div class="diff-block my-2 overflow-hidden rounded-md bg-code-bg text-xs text-code-foreground">
  <div class="flex items-center justify-between gap-2 px-2.5 py-1 text-muted-foreground">
    <span class="font-mono">diff{complete ? '' : '…'}</span>
    <button
      type="button"
      class="rounded px-1.5 py-0.5 hover:bg-muted hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
      onclick={copyPatch}
    >
      {copied ?? 'Copier'}
    </button>
  </div>
  {#each files as file, i (i)}
    <Collapsible open={true}>
      <div class="flex items-center gap-1.5 px-1.5 py-0.5">
        <CollapsibleTrigger
          class="group inline-flex items-center gap-1 rounded px-1 py-0.5 text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
          aria-label={file.path ? `Afficher ou masquer ${file.path}` : 'Afficher ou masquer le diff'}
        >
          <ChevronRight class="size-3.5 transition-transform group-data-[state=open]:rotate-90" aria-hidden="true" />
        </CollapsibleTrigger>
        {#if file.path && onFile}
          <button
            type="button"
            class="inline-flex min-w-0 items-center gap-1.5 rounded-sm font-mono focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
            onclick={() => onFile({ path: file.path! })}
          >
            <FileIcon path={file.path} />
            <span class="file-ref min-w-0 truncate">{file.path}</span>
          </button>
        {:else if file.path}
          <span class="inline-flex min-w-0 items-center gap-1.5 font-mono">
            <FileIcon path={file.path} />
            <span class="min-w-0 truncate">{file.path}</span>
          </span>
        {:else}
          <span class="font-mono">patch</span>
        {/if}
        <span class="ml-auto tabular-nums">
          <span class="text-diff-add">+{file.additions}</span>
          <span class="text-diff-del">−{file.deletions}</span>
        </span>
      </div>
      <CollapsibleContent>
        <DiffView patch={file.patch} binary={file.binary} loose class="rounded-none border-0" />
      </CollapsibleContent>
    </Collapsible>
  {/each}
</div>
