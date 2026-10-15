<script lang="ts">
  import { Ban, ChevronDown, CircleAlert, CircleCheck, CircleX, TriangleAlert } from '@lucide/svelte'
  import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '$lib/components/ui/collapsible'
  import DiffStat from '$lib/layout/DiffStat.svelte'
  import type { Conversation, RunEndItem, RunInfo } from '$lib/state/conversation.svelte'
  import type { RunOutcome } from '$lib/ipc/engine'
  import {
    OUTCOME_LABEL,
    formatDuration,
    parseTestCounts,
    plural,
    terminationLabel,
    truncate,
  } from '../format'
  import MonoBlock from './MonoBlock.svelte'

  let { item, conversation }: { item: RunEndItem; conversation: Conversation } = $props()

  const run: RunInfo | undefined = $derived(conversation.runs.get(item.runId))

  const BADGE: Record<RunOutcome, string> = {
    succeeded: 'bg-success-soft text-success',
    incomplete: 'bg-warning-soft text-warning',
    failed: 'bg-destructive/10 text-destructive',
    cancelled: 'bg-muted text-muted-foreground',
  }

  const duration = $derived(
    run && run.finishedAt !== undefined ? formatDuration(run.finishedAt - run.startedAt) : null,
  )
  const files = $derived(run ? [...run.files.values()].sort((a, b) => a.path.localeCompare(b.path)) : [])
  const reason = $derived(run?.outcome === 'incomplete' ? terminationLabel(run.termination) : null)
  const testReports = $derived(
    (run?.commands ?? []).flatMap((command) => {
      const counts = parseTestCounts(command.output)
      return counts ? [{ command: command.command, counts }] : []
    }),
  )
</script>

<section
  aria-label="Bilan de l’exécution"
  class="flex flex-col gap-3 rounded-lg border border-border bg-card p-3 text-sm text-card-foreground"
>
  <h2 class="sr-only">Bilan de l’exécution</h2>
  <header class="flex flex-wrap items-center gap-x-3 gap-y-1.5">
    {#if run?.outcome}
      <span class="inline-flex items-center gap-1.5 rounded-md px-2 py-0.5 text-xs font-medium {BADGE[run.outcome]}">
        {#if run.outcome === 'succeeded'}
          <CircleCheck class="size-3.5" aria-hidden="true" />
        {:else if run.outcome === 'incomplete'}
          <TriangleAlert class="size-3.5" aria-hidden="true" />
        {:else if run.outcome === 'failed'}
          <CircleX class="size-3.5" aria-hidden="true" />
        {:else}
          <Ban class="size-3.5" aria-hidden="true" />
        {/if}
        {OUTCOME_LABEL[run.outcome]}
      </span>
    {:else}
      <span class="inline-flex items-center gap-1.5 rounded-md bg-muted px-2 py-0.5 text-xs font-medium text-muted-foreground">
        <CircleAlert class="size-3.5" aria-hidden="true" />
        Fin de l’exécution
      </span>
    {/if}
    {#if duration}
      <span class="text-xs text-muted-foreground">Durée : <span class="tabular-nums">{duration}</span></span>
    {/if}
    {#if run?.turns !== undefined}
      <span class="text-xs text-muted-foreground">Tours : <span class="tabular-nums">{run.turns}</span></span>
    {/if}
    {#if run?.model}
      <span class="font-mono text-xs text-muted-foreground">{run.model}</span>
    {/if}
  </header>

  {#if reason}
    <p class="text-xs text-warning break-words">Incomplet : {reason}.</p>
  {/if}
  {#if run?.outcome === 'failed' && run.error}
    <p class="text-xs text-destructive break-words whitespace-pre-wrap">Erreur : {run.error}</p>
  {/if}
  {#if run?.outcome === 'cancelled'}
    <p class="text-xs text-muted-foreground">Exécution annulée avant la fin.</p>
  {/if}

  {#if run && run.approvalsUnsatisfied.length > 0}
    <div class="flex flex-col gap-1">
      <p class="text-xs font-medium text-warning">
        {plural(run.approvalsUnsatisfied.length, 'autorisation non accordée', 'autorisations non accordées')}
      </p>
      <ul class="flex flex-col gap-0.5 text-xs text-muted-foreground">
        {#each run.approvalsUnsatisfied as approval (approval.approval_id)}
          <li class="break-words">
            <span class="font-mono">{approval.tool}</span> : {truncate(approval.description, 200)}
          </li>
        {/each}
      </ul>
    </div>
  {/if}

  {#if files.length > 0}
    <section aria-label="Fichiers touchés" class="flex flex-col gap-1.5 border-t border-border pt-2.5">
      <h3 class="text-xs font-medium text-foreground">Fichiers touchés</h3>
      <ul class="flex flex-col gap-1 text-xs">
        {#each files as file (file.path)}
          <li class="flex flex-wrap items-center gap-x-2 gap-y-1">
            <span class="min-w-0 break-all font-mono">{file.path}</span>
            {#if file.binary}
              <span class="text-muted-foreground">binaire</span>
            {:else}
              <DiffStat
                additions={file.added}
                deletions={file.removed}
                label="somme des opérations d’édition, pas un diff Git net"
              />
            {/if}
          </li>
        {/each}
      </ul>
    </section>
  {/if}

  {#if testReports.length > 0}
    <section aria-label="Tests" class="flex flex-col gap-1 border-t border-border pt-2.5 text-xs">
      <h3 class="font-medium text-foreground">Tests</h3>
      {#each testReports as report, i (i)}
        <p class="break-words">
          <span class="tabular-nums">{report.counts.passed} réussi{report.counts.passed > 1 ? 's' : ''}</span>
          /
          <span class="tabular-nums {report.counts.failed > 0 ? 'text-destructive' : ''}">
            {report.counts.failed} échoué{report.counts.failed > 1 ? 's' : ''}
          </span>
          <span class="text-muted-foreground">
            ({report.counts.framework}, commande <span class="font-mono">{truncate(report.command, 120)}</span>)
          </span>
        </p>
      {/each}
    </section>
  {/if}

  {#if run && run.commands.length > 0}
    <section aria-label="Commandes exécutées" class="flex flex-col gap-1.5 border-t border-border pt-2.5">
      <h3 class="text-xs font-medium text-foreground">
        {plural(run.commands.length, 'commande exécutée', 'commandes exécutées')}
      </h3>
      <ul class="flex flex-col gap-1.5">
        {#each run.commands as command (command.toolCallId)}
          <li class="rounded-md border border-border text-xs">
            <Collapsible>
              <CollapsibleTrigger
                class="group flex w-full min-w-0 items-center gap-2 rounded-md px-2 py-1.5 text-left hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
              >
                <ChevronDown class="size-3.5 shrink-0 text-muted-foreground transition-transform group-data-[state=open]:rotate-180" aria-hidden="true" />
                {#if command.isError}
                  <CircleX class="size-3.5 shrink-0 text-destructive" aria-hidden="true" />
                {:else}
                  <CircleCheck class="size-3.5 shrink-0 text-success" aria-hidden="true" />
                {/if}
                <span class="min-w-0 flex-1 truncate font-mono">{command.command || command.name}</span>
                <span class="shrink-0 {command.isError ? 'text-destructive' : 'text-muted-foreground'}">
                  {command.isError ? 'Échouée' : 'Réussie'}
                </span>
                <span class="shrink-0 whitespace-nowrap text-muted-foreground tabular-nums">{formatDuration(command.durationMs)}</span>
              </CollapsibleTrigger>
              <CollapsibleContent class="border-t border-border p-2">
                <MonoBlock text={command.output || '(aucune sortie)'} label={`Sortie de ${truncate(command.command || command.name, 80)}`} />
              </CollapsibleContent>
            </Collapsible>
          </li>
        {/each}
      </ul>
    </section>
  {/if}
</section>
