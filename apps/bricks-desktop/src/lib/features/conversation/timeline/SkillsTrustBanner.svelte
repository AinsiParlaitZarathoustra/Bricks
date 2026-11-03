<script lang="ts">
  import { t } from '$lib/i18n/i18n.svelte'
  import ShieldAlert from '@lucide/svelte/icons/shield-alert'
  import { Button } from '$lib/components/ui/button'
  import { app } from '$lib/state/app.svelte'
  import type { Conversation } from '$lib/state/conversation.svelte'
  import type { Command } from '$lib/ipc/engine'

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
  aria-label={t('common.projectSkills')}
  class="flex flex-col gap-2.5 rounded-lg border border-warning/40 bg-card p-3 text-sm text-card-foreground"
>
  <header class="flex items-center gap-2">
    <ShieldAlert class="size-4 shrink-0 text-warning" aria-hidden="true" />
    <h2 class="font-medium">{t('conversation.projectSkillsToAllow')}</h2>
  </header>
  <p class="text-xs text-muted-foreground break-words">
    {t('conversation.thisProjectProvidesSkillsUntilThey')}
  </p>
  <ul class="flex flex-col gap-1 text-xs">
    {#each trust.sources as source, i (i)}
      <li class="flex flex-wrap items-baseline gap-x-2 gap-y-0.5">
        <span class="font-medium">{source.source}</span>
        <span class="min-w-0 break-all font-mono text-muted-foreground">{source.path}</span>
        <span class="text-muted-foreground">{t('common.skillCount', { count: source.skills })}</span>
      </li>
    {/each}
  </ul>
  <div class="flex flex-wrap gap-2">
    <Button size="sm" disabled={sending !== null} onclick={() => answer(true)}>{t('conversation.allowProjectSkills')}</Button>
    <Button size="sm" variant="outline" disabled={sending !== null} onclick={() => answer(false)}>{t('common.refuse')}</Button>
  </div>
  {#if sending !== null && refusal === null}
    <p class="text-xs text-muted-foreground" role="status">{t('conversation.answerSent')}</p>
  {/if}
  {#if refusal}
    <p class="text-xs text-destructive break-words" role="alert">{refusal}</p>
  {/if}
</section>
