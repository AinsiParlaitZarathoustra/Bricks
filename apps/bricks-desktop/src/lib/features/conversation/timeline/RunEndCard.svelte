<script lang="ts">
  // End of a response: its outcome and working time, then two independent
  // groups, closed by default: the files changed (cumulative counts of the
  // applied operations, names only; a name opens the reader) and the
  // commands run by the response and its sub-agents.
  import Ban from '@lucide/svelte/icons/ban'
  import ChevronDown from '@lucide/svelte/icons/chevron-down'
  import CircleAlert from '@lucide/svelte/icons/circle-alert'
  import CircleCheck from '@lucide/svelte/icons/circle-check'
  import CircleDashed from '@lucide/svelte/icons/circle-dashed'
  import CircleX from '@lucide/svelte/icons/circle-x'
  import TriangleAlert from '@lucide/svelte/icons/triangle-alert'
  import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '$lib/components/ui/collapsible'
  import DiffStat from '$lib/layout/DiffStat.svelte'
  import type { CommandEvidence, Conversation, RunEndItem, RunFileSummary, RunInfo } from '$lib/state/conversation.svelte'
  import type { RunOutcome } from '$lib/ipc/engine'
  import { reader } from '$lib/state/reader.svelte'
  import { formatDuration, parseTestCounts, plural, truncate } from '../format'
  import { formatWorked, runEndSummary } from '../runend'
  import MonoBlock from './MonoBlock.svelte'

  let { item, conversation }: { item: RunEndItem; conversation: Conversation } = $props()

  const run: RunInfo | undefined = $derived(conversation.runs.get(item.runId))
  const summary = $derived(runEndSummary(run))
  const worked = $derived(formatWorked(run?.workedMs ?? null))

  const TONE: Record<RunOutcome, string> = {
    succeeded: 'text-success',
    incomplete: 'text-warning',
    failed: 'text-destructive',
    cancelled: 'text-muted-foreground',
  }

  const files = $derived(
    run ? [...run.fileSummaries].sort((a, b) => a.name.localeCompare(b.name) || a.path.localeCompare(b.path)) : [],
  )
  const commands = $derived(run?.commands ?? [])
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

  const STATUS: Record<CommandEvidence['status'], { label: string; tone: string }> = {
    success: { label: 'Réussie', tone: 'text-muted-foreground' },
    failure: { label: 'Échouée', tone: 'text-destructive' },
    timed_out: { label: 'Délai dépassé', tone: 'text-destructive' },
    cancelled: { label: 'Interrompue', tone: 'text-muted-foreground' },
    running: { label: 'En cours', tone: 'text-muted-foreground' },
    unknown: { label: 'État inconnu', tone: 'text-muted-foreground' },
  }

  function openFile(file: RunFileSummary) {
    reader.open({
      sessionId: conversation.sessionId,
      projectId: conversation.projectId,
      agentId: file.agentIds[0] ?? null,
      path: file.path,
      absolute: file.absolute,
    })
  }

  const trigger =
    'group flex w-full min-w-0 items-center gap-1.5 rounded-md px-1.5 py-1 text-start text-[13px] hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none'
  const chevron = 'size-3.5 shrink-0 -rotate-90 text-muted-foreground transition-transform group-data-[state=open]:rotate-0'
</script>

<section
  aria-label="Bilan de la réponse"
  class="flex flex-col gap-1 rounded-lg border border-border bg-white px-2 py-1.5 text-sm text-card-foreground dark:bg-card"
>
  <p class="flex flex-wrap items-center gap-x-2 gap-y-0.5 px-1.5 py-0.5 text-[13px]">
    <span class="inline-flex items-center gap-1.5 font-medium {summary.outcome ? TONE[summary.outcome] : 'text-muted-foreground'}">
      {#if summary.outcome === 'succeeded'}
        <CircleCheck class="size-3.5" aria-hidden="true" />
      {:else if summary.outcome === 'incomplete'}
        <TriangleAlert class="size-3.5" aria-hidden="true" />
      {:else if summary.outcome === 'failed'}
        <CircleX class="size-3.5" aria-hidden="true" />
      {:else if summary.outcome === 'cancelled'}
        <Ban class="size-3.5" aria-hidden="true" />
      {:else}
        <CircleAlert class="size-3.5" aria-hidden="true" />
      {/if}
      {summary.label}
    </span>
    {#if worked}
      <span aria-hidden="true" class="text-muted-foreground">·</span>
      <span class="text-muted-foreground tabular-nums">A travaillé {worked}</span>
    {/if}
  </p>

  {#if summary.reason}
    <p class="px-1.5 text-xs break-words text-warning">Motif : {summary.reason}.</p>
  {/if}
  {#if summary.error}
    <p class="px-1.5 text-xs break-words whitespace-pre-wrap text-destructive">Erreur : {summary.error}</p>
  {/if}
  {#if run && run.approvalsUnsatisfied.length > 0}
    <p class="px-1.5 text-xs text-warning">
      {plural(run.approvalsUnsatisfied.length, 'autorisation non accordée', 'autorisations non accordées')}
    </p>
  {/if}

  {#if files.length > 0}
    <Collapsible>
      <CollapsibleTrigger class={trigger}>
        <ChevronDown class={chevron} aria-hidden="true" />
        <span class="font-medium">{plural(files.length, 'fichier modifié', 'fichiers modifiés')}</span>
        {#if totals}
          <span title="Somme des opérations d’édition appliquées (pas un diff Git net)">
            <DiffStat additions={totals.added} deletions={totals.removed} class="text-[13px]" />
          </span>
        {/if}
      </CollapsibleTrigger>
      <CollapsibleContent>
        <ul class="flex flex-col py-0.5 pl-6 text-[13px]">
          {#each files as file (file.key)}
            <li class="flex min-w-0 items-center gap-2 py-0.5">
              <button
                type="button"
                class="min-w-0 truncate rounded-sm text-start font-mono text-xs hover:underline focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                title={file.absolute ?? file.path}
                onclick={() => openFile(file)}
              >
                {file.name}
              </button>
              {#if file.binary}
                <span class="text-xs text-muted-foreground">binaire</span>
              {:else}
                <span title="Somme des opérations d’édition appliquées (pas un diff Git net)">
                  <DiffStat additions={file.added} deletions={file.removed} />
                </span>
              {/if}
            </li>
          {/each}
        </ul>
      </CollapsibleContent>
    </Collapsible>
  {/if}

  {#if commands.length > 0}
    <Collapsible>
      <CollapsibleTrigger class={trigger}>
        <ChevronDown class={chevron} aria-hidden="true" />
        <span class="font-medium">{plural(commands.length, 'commande exécutée', 'commandes exécutées')}</span>
      </CollapsibleTrigger>
      <CollapsibleContent>
        <ul class="flex flex-col gap-0.5 py-0.5 pl-6">
          {#each commands as command (command.key)}
            {@const status = STATUS[command.status]}
            {@const tests = command.output ? parseTestCounts(command.output) : null}
            <li class="text-xs">
              <Collapsible>
                <CollapsibleTrigger
                  class="group flex w-full min-w-0 items-center gap-2 rounded-md px-1.5 py-1 text-start hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                >
                  <ChevronDown class={chevron} aria-hidden="true" />
                  {#if command.status === 'success'}
                    <CircleCheck class="size-3.5 shrink-0 text-success" aria-hidden="true" />
                  {:else if command.status === 'running' || command.status === 'unknown'}
                    <CircleDashed class="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
                  {:else if command.status === 'cancelled'}
                    <Ban class="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
                  {:else}
                    <CircleX class="size-3.5 shrink-0 text-destructive" aria-hidden="true" />
                  {/if}
                  <span class="min-w-0 flex-1 truncate font-mono">{command.command || command.name}</span>
                  <span class="shrink-0 {status.tone}">{status.label}</span>
                  {#if command.durationMs !== null}
                    <span class="shrink-0 whitespace-nowrap text-muted-foreground tabular-nums">{formatDuration(command.durationMs)}</span>
                  {/if}
                </CollapsibleTrigger>
                <CollapsibleContent class="flex flex-col gap-1 py-1 pl-6">
                  {#if tests}
                    <p class="text-muted-foreground">
                      <span class="tabular-nums">{tests.passed} réussi{tests.passed > 1 ? 's' : ''}</span>
                      /
                      <span class="tabular-nums {tests.failed > 0 ? 'text-destructive' : ''}">
                        {tests.failed} échoué{tests.failed > 1 ? 's' : ''}
                      </span>
                      ({tests.framework})
                    </p>
                  {/if}
                  {#if command.output}
                    <MonoBlock text={command.output} label={`Sortie de ${truncate(command.command || command.name, 80)}`} />
                  {:else}
                    <p class="text-muted-foreground">Sortie non disponible.</p>
                  {/if}
                </CollapsibleContent>
              </Collapsible>
            </li>
          {/each}
        </ul>
      </CollapsibleContent>
    </Collapsible>
  {/if}
</section>
