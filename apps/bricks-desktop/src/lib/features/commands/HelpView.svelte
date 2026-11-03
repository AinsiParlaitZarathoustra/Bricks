<script lang="ts">
  // `/help`: the commands come from the engine registry (the same list as the
  // palette, nothing is written here); the keys are the desktop interactions.
  import { t } from '$lib/i18n/i18n.svelte'
  import { onMount } from 'svelte'
  import { Button } from '$lib/components/ui/button'
  import { Kbd } from '$lib/components/ui/kbd'
  import type { InspectorState } from '$lib/state/ui.svelte'
  import { commandDescription, slashRegistry } from './slash-registry.svelte'
  import { usageLine } from './palette'

  // The host passes `{ args }`; this view needs none of them.
  let _props: { args?: InspectorState['args'] } = $props()

  onMount(() => {
    void slashRegistry.load()
  })

  const interactions: { keys: string[]; text: string }[] = $derived([
    { keys: [t('common.enter')], get text() { return t('commands.sendsTheMessageTextStartingWith') } },
    { keys: [t('commands.shift'), t('common.enter')], get text() { return t('commands.newLineInTheMessage') } },
    { keys: [t('commands.cmd'), t('common.enter')], get text() { return t('commands.alsoSendsTheMessageCtrlEnter') } },
    { keys: ['/'], get text() { return t('commands.atTheStartOfTheMessage') } },
    { keys: ['↑', '↓'], get text() { return t('commands.inTheCommandListChoosesA') } },
    { keys: [t('commands.tab')], get text() { return t('commands.inTheListCompletesTheCommand') } },
    { keys: [t('common.enter')], get text() { return t('commands.inTheListCompletesAPartial') } },
    { keys: [t('commands.esc')], get text() { return t('commands.closesTheCommandListWithoutClearing') } },
    { keys: ['↑'], get text() { return t('commands.inAnEmptyMessageOrWith') } },
    { keys: ['↓'], get text() { return t('commands.goesBackToMoreRecentRequests') } },
  ])
</script>

<div class="flex flex-col gap-5 text-sm">
  <section class="flex flex-col gap-2" aria-labelledby="help-commands">
    <h3 id="help-commands" class="text-sm font-semibold">{t('commands.commands')}</h3>

    {#if slashRegistry.state.status === 'loading'}
      <p role="status" class="text-muted-foreground">{t('commands.loadingCommands')}</p>
    {:else if slashRegistry.state.status === 'error'}
      <div role="alert" class="flex flex-col items-start gap-2">
        <p class="text-destructive">{t('commands.commandsUnavailable', { error: slashRegistry.state.message })}</p>
        <Button variant="outline" size="sm" onclick={() => slashRegistry.retry()}>{t('common.retry')}</Button>
      </div>
    {:else}
      <ul class="flex flex-col divide-y">
        {#each slashRegistry.state.commands as cmd (cmd.name)}
          <li class="flex flex-col gap-0.5 py-2">
            <span class="font-mono font-medium">{usageLine(cmd)}</span>
            {#if cmd.aliases.length > 0}
              <span class="text-xs text-muted-foreground">
                {t('commands.alias', { value: cmd.aliases.map((a) => `/${a}`).join(', ') })}
              </span>
            {/if}
            <span class="text-muted-foreground">{commandDescription(cmd)}</span>
          </li>
        {/each}
      </ul>
    {/if}
  </section>

  <section class="flex flex-col gap-2" aria-labelledby="help-keys">
    <h3 id="help-keys" class="text-sm font-semibold">{t('commands.keyboardAndInteractions')}</h3>
    <ul class="flex flex-col gap-2">
      {#each interactions as item, i (i)}
        <li class="flex items-start gap-3">
          <span class="flex shrink-0 flex-wrap gap-1 pt-0.5">
            {#each item.keys as key, k (k)}
              <Kbd>{key}</Kbd>
            {/each}
          </span>
          <span class="text-muted-foreground">{item.text}</span>
        </li>
      {/each}
    </ul>
    <p class="text-muted-foreground">
      {t('commands.aGenerationEndsWithTheStop')}
    </p>
  </section>
</div>
