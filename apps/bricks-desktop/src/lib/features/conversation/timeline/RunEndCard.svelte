<script lang="ts">
  // End of a response, under its answer: why it did not succeed, when it did not,
  // then the files it changed (cumulative counts of the applied operations). Each
  // file opens its changes in the reader, or shows itself in the Finder. The time
  // worked is in the bar of its steps (WorkBlock.svelte).
  import ChevronRight from '@lucide/svelte/icons/chevron-right'
  import ExternalLink from '@lucide/svelte/icons/external-link'
  import Eye from '@lucide/svelte/icons/eye'
  import { Button } from '$lib/components/ui/button'
  import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '$lib/components/ui/collapsible'
  import FileIcon from '$lib/features/file-icons/FileIcon.svelte'
  import DiffStat from '$lib/layout/DiffStat.svelte'
  import { readerFileReveal } from '$lib/ipc/reader'
  import type { Conversation, RunEndItem, RunFileSummary, RunInfo } from '$lib/state/conversation.svelte'
  import { reader } from '$lib/state/reader.svelte'
  import { plural } from '../format'
  import { runEndSummary } from '../runend'

  let { item, conversation }: { item: RunEndItem; conversation: Conversation } = $props()

  const run: RunInfo | undefined = $derived(conversation.runs.get(item.runId))
  const summary = $derived(runEndSummary(run))

  const files = $derived(
    run ? [...run.fileSummaries].sort((a, b) => a.name.localeCompare(b.name) || a.path.localeCompare(b.path)) : [],
  )
  const totals = $derived.by(() => {
    if (files.length === 0 || files.every((f) => f.binary)) return null
    let added = 0
    let removed = 0
    for (const f of files) {
      added += f.added
      removed += f.removed
    }
    return { added, removed }
  })
  const unsatisfied = $derived(run?.approvalsUnsatisfied.length ?? 0)
  const reason = $derived(summary.outcome === 'incomplete' ? summary.reason : null)

  let open = $state(true)
  /** Error of the last « Afficher dans le Finder »; cleared by the next one. */
  let revealError = $state<string | null>(null)

  /**
   * The folder of a file: its relative folder in the workspace, or only the name of
   * its parent folder for an absolute path (the full path is in the title). Empty at the root.
   */
  function folderOf(file: RunFileSummary): string {
    const path = file.path
    const cut = path.lastIndexOf('/')
    if (cut <= 0) return ''
    const folder = path.slice(0, cut)
    return path.startsWith('/') ? folder.slice(folder.lastIndexOf('/') + 1) : folder
  }

  function review(file: RunFileSummary) {
    const tab = reader.open({
      sessionId: conversation.sessionId,
      projectId: conversation.projectId,
      agentId: file.agentIds[0] ?? null,
      path: file.path,
      absolute: file.absolute,
    })
    // Falls back to the source while the file offers no changes.
    tab.requested = 'changes'
  }

  async function reveal(file: RunFileSummary) {
    revealError = null
    try {
      await readerFileReveal(conversation.sessionId, file.agentIds[0] ?? null, file.absolute ?? file.path)
    } catch (e) {
      revealError = e instanceof Error ? e.message : String(e)
    }
  }
</script>

{#if reason || summary.error || unsatisfied > 0 || files.length > 0}
<div class="flex flex-col gap-2">
  {#if reason}
    <p class="text-xs break-words text-warning">Motif : {reason}.</p>
  {/if}
  {#if summary.error}
    <p class="text-xs break-words whitespace-pre-wrap text-destructive">Erreur : {summary.error}</p>
  {/if}
  {#if unsatisfied > 0}
    <p class="text-xs text-warning">
      {plural(unsatisfied, 'autorisation non accordée', 'autorisations non accordées')}
    </p>
  {/if}

  {#if files.length > 0}
    <section aria-label="Fichiers modifiés" class="rounded-xl border border-border bg-card text-ui text-card-foreground">
      <Collapsible bind:open>
        <CollapsibleTrigger
          class="group flex w-full min-w-0 items-center gap-2 rounded-xl px-3 py-2 text-start hover:bg-muted/50 focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
        >
          <ChevronRight
            class="size-3.5 shrink-0 text-muted-foreground transition-transform group-data-[state=open]:rotate-90"
            aria-hidden="true"
          />
          <span class="font-medium">{plural(files.length, 'fichier modifié', 'fichiers modifiés')}</span>
          {#if totals}
            <DiffStat additions={totals.added} deletions={totals.removed} class="text-ui" />
          {/if}
        </CollapsibleTrigger>
        <CollapsibleContent>
          <ul class="border-t border-border">
            {#each files as file (file.key)}
              {@const folder = folderOf(file)}
              <li class="flex min-w-0 items-center gap-2 border-b border-border px-3 py-1.5 last:border-b-0">
                <FileIcon path={file.path} />
                <span class="min-w-0 shrink-0 truncate font-medium" title={file.absolute ?? file.path}>{file.name}</span>
                {#if folder}
                  <span class="min-w-0 truncate text-xs text-muted-foreground">{folder}/</span>
                {/if}
                {#if file.binary}
                  <span class="shrink-0 text-xs text-muted-foreground">binaire</span>
                {:else}
                  <DiffStat additions={file.added} deletions={file.removed} class="shrink-0" />
                {/if}
                <span class="ml-auto flex shrink-0 items-center gap-0.5">
                  <Button
                    variant="ghost"
                    size="icon-xs"
                    aria-label={`Voir les modifications de ${file.name}`}
                    title="Voir les modifications"
                    onclick={() => review(file)}
                  >
                    <Eye aria-hidden="true" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon-xs"
                    aria-label={`Afficher ${file.name} dans le Finder`}
                    title="Afficher dans le Finder"
                    onclick={() => reveal(file)}
                  >
                    <ExternalLink aria-hidden="true" />
                  </Button>
                </span>
              </li>
            {/each}
          </ul>
        </CollapsibleContent>
      </Collapsible>
      {#if revealError}
        <p role="alert" class="border-t border-border px-3 py-1.5 text-xs break-words text-destructive">{revealError}</p>
      {/if}
    </section>
  {/if}
</div>
{/if}
