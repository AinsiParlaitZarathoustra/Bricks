<script lang="ts">
  import ChevronDown from '@lucide/svelte/icons/chevron-down'
  import ShieldAlert from '@lucide/svelte/icons/shield-alert'
  import ShieldCheck from '@lucide/svelte/icons/shield-check'
  import ShieldX from '@lucide/svelte/icons/shield-x'
  import { Button } from '$lib/components/ui/button'
  import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '$lib/components/ui/collapsible'
  import DiffStat from '$lib/layout/DiffStat.svelte'
  import { app } from '$lib/state/app.svelte'
  import type { Conversation } from '$lib/state/conversation.svelte'
  import type { ApprovalRequest, Command, DecidedBy, Decision } from '$lib/ipc/engine'
  import { CHANGE_KIND_LABEL, prettyJson, summarizeInput } from '../format'
  import MonoBlock from './MonoBlock.svelte'
  import { approvalTrace, sessionGrantOffered } from './toolLine'

  interface Props {
    approval: ApprovalRequest
    resolved?: { decision: Decision; by: DecidedBy }
    conversation: Conversation
  }

  let { approval, resolved, conversation }: Props = $props()

  /** Decision being sent; kept until the engine confirms with approval_resolved. */
  let sending = $state<Decision | null>(null)
  let refusal = $state<string | null>(null)

  const agentName = $derived.by(() => {
    if (!approval.agent_id) return null
    const agent = conversation.agents.get(approval.agent_id)
    return agent ? agent.spawn.profile : `agent inconnu (${approval.agent_id.slice(0, 8)})`
  })
  const inputSummary = $derived(summarizeInput(approval.tool, approval.input))
  const inputText = $derived(prettyJson(approval.input))
  const grantOffered = $derived(sessionGrantOffered(app.prefs.tool_approval))

  async function decide(decision: Decision) {
    if (sending !== null || resolved) return
    if (decision === 'allow_for_session' && !grantOffered) return
    sending = decision
    refusal = null
    const message: string | null = await app.command(conversation.sessionId, {
      type: 'approve',
      approval_id: approval.approval_id,
      decision,
    } satisfies Command)
    if (message) {
      refusal = message
      sending = null
    }
  }
</script>

{#if resolved}
  <p class="flex min-w-0 items-center gap-2 px-2.5 py-0.5 text-xs text-muted-foreground">
    {#if resolved.decision === 'deny'}
      <ShieldX class="size-3.5 shrink-0 text-destructive" aria-hidden="true" />
    {:else}
      <ShieldCheck class="size-3.5 shrink-0 text-success" aria-hidden="true" />
    {/if}
    <span class="min-w-0 break-words">{approvalTrace(approval.tool, resolved)}</span>
  </p>
{:else}
  <section
    aria-label="Autorisation demandée"
    class="flex flex-col gap-2.5 rounded-lg border border-warning/40 bg-card p-3 text-sm text-card-foreground"
  >
    <header class="flex flex-wrap items-center gap-x-2 gap-y-1">
      <ShieldAlert class="size-4 shrink-0 text-warning" aria-hidden="true" />
      <h2 class="font-medium">Autorisation demandée</h2>
      <span class="rounded bg-muted px-1.5 py-0.5 font-mono text-xs">{approval.tool}</span>
      <span class="text-xs text-muted-foreground">Niveau : {approval.level}</span>
      {#if agentName}
        <span class="text-xs text-muted-foreground">Agent : {agentName}</span>
      {/if}
    </header>

    <p class="break-words">{approval.description}</p>

    {#if inputSummary}
      <p class="font-mono text-xs break-all text-muted-foreground">{inputSummary}</p>
    {/if}

    <Collapsible>
      <CollapsibleTrigger
        class="group inline-flex w-fit items-center gap-1 rounded-md text-xs text-muted-foreground hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
      >
        <ChevronDown class="size-3.5 transition-transform group-data-[state=open]:rotate-180" aria-hidden="true" />
        Entrée complète
      </CollapsibleTrigger>
      <CollapsibleContent class="mt-1.5">
        <MonoBlock text={inputText} label={`Entrée complète de ${approval.tool}`} />
      </CollapsibleContent>
    </Collapsible>

    {#if approval.preview}
      {#if approval.preview.refusal}
        <p class="text-xs text-destructive break-words">{approval.preview.refusal}</p>
      {/if}
      {#if approval.preview.files.length > 0}
        <ul class="flex flex-col gap-2 border-t border-border pt-2.5" aria-label="Fichiers concernés">
          {#each approval.preview.files as file (file.absolute)}
            <li class="flex flex-col gap-1.5">
              <Collapsible>
                <div class="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs">
                  <span class="text-muted-foreground">{CHANGE_KIND_LABEL[file.kind]}</span>
                  <span class="min-w-0 break-all font-mono">{file.path}</span>
                  <DiffStat additions={file.added} deletions={file.removed} label="lignes" />
                  {#if file.diff}
                    <CollapsibleTrigger
                      aria-label={`Voir le diff de ${file.path}`} class="rounded px-1 text-primary underline-offset-4 hover:underline focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                    >
                      Voir le diff
                    </CollapsibleTrigger>
                  {/if}
                </div>
                {#if file.diff}
                  <CollapsibleContent class="mt-1.5">
                    <MonoBlock text={file.diff} label={`Diff de ${file.path}`} diff maxLines={400} />
                  </CollapsibleContent>
                {/if}
              </Collapsible>
            </li>
          {/each}
        </ul>
      {/if}
    {/if}

    <div class="flex flex-wrap gap-2">
      <Button size="sm" disabled={sending !== null} onclick={() => decide('allow')}>Autoriser</Button>
      {#if grantOffered}
        <Button size="sm" variant="outline" disabled={sending !== null} onclick={() => decide('allow_for_session')}>
          Autoriser pour la session
        </Button>
      {/if}
      <Button size="sm" variant="ghost" class="text-destructive hover:text-destructive" disabled={sending !== null} onclick={() => decide('deny')}>
        Refuser
      </Button>
    </div>
    {#if sending !== null && refusal === null}
      <p class="text-xs text-muted-foreground" role="status">Décision envoyée…</p>
    {/if}
    {#if refusal}
      <p class="text-xs text-destructive break-words" role="alert">{refusal}</p>
    {/if}
  </section>
{/if}
