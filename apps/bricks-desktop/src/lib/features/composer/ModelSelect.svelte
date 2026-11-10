<script lang="ts">
  // The model and its reasoning level, in one control: the model's name, then the level in a
  // quieter tone. The menu lists the models by provider; a model with several levels opens them
  // to its right, and picking a level picks the model with it and closes the menu.
  import { t } from '$lib/i18n/i18n.svelte'
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu'
  import CheckIcon from '@lucide/svelte/icons/check'
  import ChevronDownIcon from '@lucide/svelte/icons/chevron-down'
  import type { ModelChoice } from '$lib/ipc/engine'

  let {
    open = $bindable(false),
    value,
    level,
    models,
    disabled = false,
    onPick,
  }: {
    /** Opened from outside (`/model`, `/effort`); the menu reports its own state. */
    open?: boolean
    /** Selected `provider/model`, or `null` when none is chosen yet. */
    value: string | null
    /** The level in force for the selected model, `null` when it has none. */
    level: string | null
    models: ModelChoice[]
    disabled?: boolean
    /** Resolves with the refusal message, or `null` when the change is accepted. */
    onPick: (selection: string, level: string | null) => Promise<string | null>
  } = $props()

  /** Shown choice: follows the props, shows a pick at once, reverts if refused. */
  let shown = $derived({ selection: value ?? '', level })

  /** Models grouped by provider, in catalogue order. */
  const groups = $derived.by(() => {
    const out: { provider: string; items: ModelChoice[] }[] = []
    for (const m of models) {
      const group = out.find((g) => g.provider === m.provider)
      if (group) group.items.push(m)
      else out.push({ provider: m.provider, items: [m] })
    }
    return out
  })

  const current = $derived(models.find((m) => m.selection === shown.selection))

  function levelLabel(model: ModelChoice | undefined, id: string | null): string {
    if (!model || !id) return ''
    return model.profiles.find((p) => p.id === id)?.label ?? id
  }

  /** The level a model would run with: the one in force for the selected model, else its first. */
  function levelOf(model: ModelChoice): string | null {
    if (model.selection === shown.selection && shown.level) return shown.level
    return model.default_profile ?? model.profiles[0]?.id ?? null
  }

  const name = $derived(
    models.length === 0 ? t('composer.noModelConfigured2') : (current?.name ?? t('composer.chooseAModel')),
  )
  const currentLevel = $derived(current ? levelLabel(current, levelOf(current)) : '')

  async function pick(selection: string, next: string | null) {
    if (selection === shown.selection && next === shown.level) return
    const before = shown
    shown = { selection, level: next }
    const refusal = await onPick(selection, next)
    if (refusal) shown = before
  }
</script>

<DropdownMenu.Root bind:open>
  <DropdownMenu.Trigger
    disabled={disabled || models.length === 0}
    class="group/model inline-flex h-control-md max-w-72 min-w-0 items-center gap-1.5 rounded-lg px-2 text-ui text-foreground outline-none hover:bg-element-hover focus-visible:ring-2 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 data-[state=open]:bg-element-selected"
    aria-label={t('composer.model', { label: currentLevel ? `${name} ${currentLevel}` : name })}
  >
    <span class="truncate">{name}</span>
    {#if currentLevel}
      <span class="shrink-0 text-muted-foreground">{currentLevel}</span>
    {/if}
    <ChevronDownIcon
      class="size-3.5 shrink-0 text-muted-foreground transition-transform duration-150 group-data-[state=open]/model:rotate-180"
      aria-hidden="true"
    />
  </DropdownMenu.Trigger>
  <DropdownMenu.Content side="top" align="start" class="w-auto min-w-64 rounded-xl p-1.5 shadow-float">
    {#each groups as group, index (group.provider)}
      {#if index > 0}
        <DropdownMenu.Separator />
      {/if}
      <DropdownMenu.Group>
        <DropdownMenu.GroupHeading class="px-2 pt-1.5 pb-1 text-caption font-medium text-muted-foreground">
          {group.provider}
        </DropdownMenu.GroupHeading>
        {#each group.items as model (model.selection)}
          {@const selected = model.selection === shown.selection}
          {@const modelLevel = levelOf(model)}
          {#if model.profiles.length > 1}
            <DropdownMenu.Sub>
              <DropdownMenu.SubTrigger class="h-8 gap-2 rounded-lg px-2 text-ui">
                <span class="truncate">{model.name}</span>
                <span class="shrink-0 text-muted-foreground">{levelLabel(model, modelLevel)}</span>
                {#if selected}
                  <CheckIcon class="ml-auto size-4 shrink-0" aria-hidden="true" />
                {/if}
              </DropdownMenu.SubTrigger>
              <DropdownMenu.SubContent class="min-w-40 rounded-xl p-1.5 shadow-float">
                {#each model.profiles as profile (profile.id)}
                  <DropdownMenu.Item
                    class="h-8 gap-2 rounded-lg px-2 text-ui"
                    onSelect={() => void pick(model.selection, profile.id)}
                  >
                    <span class="truncate">{profile.label}</span>
                    {#if selected && profile.id === modelLevel}
                      <CheckIcon class="ml-auto size-4 shrink-0" aria-hidden="true" />
                    {/if}
                  </DropdownMenu.Item>
                {/each}
              </DropdownMenu.SubContent>
            </DropdownMenu.Sub>
          {:else}
            <DropdownMenu.Item
              class="h-8 gap-2 rounded-lg px-2 text-ui"
              onSelect={() => void pick(model.selection, model.profiles[0]?.id ?? null)}
            >
              <span class="truncate">{model.name}</span>
              {#if modelLevel}
                <span class="shrink-0 text-muted-foreground">{levelLabel(model, modelLevel)}</span>
              {/if}
              {#if selected}
                <CheckIcon class="ml-auto size-4 shrink-0" aria-hidden="true" />
              {/if}
            </DropdownMenu.Item>
          {/if}
        {/each}
      </DropdownMenu.Group>
    {/each}
  </DropdownMenu.Content>
</DropdownMenu.Root>
