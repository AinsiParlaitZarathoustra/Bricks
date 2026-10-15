<script lang="ts">
  import { ShieldAlert } from '@lucide/svelte'
  import { Button } from '$lib/components/ui/button'
  import { app } from '$lib/state/app.svelte'
  import type { Conversation } from '$lib/state/conversation.svelte'
  import type { Command } from '$lib/ipc/engine'
  import { plural } from '../format'

  let { conversation, trust }: { conversation: Conversation; trust: NonNullable<Conversation['skillsTrust']> } =
    $props()

  let sending = $state<boolean | null>(null)
  let refusal = $state<string | null>(null)

  async function answer(allow: boolean) {
    if (sending !== null) return
    sending = allow
    refusal = null
    const message: string | null = await app.command(conversation.sessionId, {
      type: 'trust_project_skills',
      allow,
    } satisfies Command)
    if (message) {
      refusal = message
      sending = null
    }
  }
</script>

<section
  aria-label="Skills du projet"
  class="flex flex-col gap-2.5 rounded-lg border border-warning/40 bg-card p-3 text-sm text-card-foreground"
>
  <header class="flex items-center gap-2">
    <ShieldAlert class="size-4 shrink-0 text-warning" aria-hidden="true" />
    <h2 class="font-medium">Skills du projet à autoriser</h2>
  </header>
  <p class="text-xs text-muted-foreground break-words">
    Ce projet fournit des skills. Tant qu’ils ne sont pas autorisés, l’agent ne les utilise pas.
  </p>
  <ul class="flex flex-col gap-1 text-xs">
    {#each trust.sources as source, i (i)}
      <li class="flex flex-wrap items-baseline gap-x-2 gap-y-0.5">
        <span class="font-medium">{source.source}</span>
        <span class="min-w-0 break-all font-mono text-muted-foreground">{source.path}</span>
        <span class="text-muted-foreground">{plural(source.skills, 'skill')}</span>
      </li>
    {/each}
  </ul>
  <div class="flex flex-wrap gap-2">
    <Button size="sm" disabled={sending !== null} onclick={() => answer(true)}>Autoriser les skills du projet</Button>
    <Button size="sm" variant="outline" disabled={sending !== null} onclick={() => answer(false)}>Refuser</Button>
  </div>
  {#if sending !== null && refusal === null}
    <p class="text-xs text-muted-foreground" role="status">Réponse envoyée…</p>
  {/if}
  {#if refusal}
    <p class="text-xs text-destructive break-words" role="alert">{refusal}</p>
  {/if}
</section>
