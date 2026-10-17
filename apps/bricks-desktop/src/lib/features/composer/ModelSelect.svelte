<script lang="ts">
  import * as Select from '$lib/components/ui/select'
  import type { ModelChoice } from '$lib/ipc/engine'

  let {
    open = $bindable(false),
    value,
    models,
    disabled = false,
    onChange,
  }: {
    /** Opened from outside (`/model`); the select reports its own state. */
    open?: boolean
    /** Selected `provider/model`, or `null` when none is chosen yet. */
    value: string | null
    models: ModelChoice[]
    disabled?: boolean
    /** Resolves with the refusal message, or `null` when the change is accepted. */
    onChange: (selection: string) => Promise<string | null>
  } = $props()

  /** Shown choice: follows `value`, shows the pick at once, reverts if refused. */
  let shown = $derived(value ?? '')

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

  const current = $derived(models.find((m) => m.selection === shown))
  const label = $derived(
    models.length === 0 ? 'Aucun modèle configuré' : (current?.name ?? (shown || 'Choisir un modèle')),
  )

  async function handleChange(next: string | undefined) {
    if (!next || next === value) return
    shown = next
    const refusal = await onChange(next)
    if (refusal) shown = value ?? ''
  }
</script>

<Select.Root
  bind:open
  type="single"
  value={shown}
  onValueChange={handleChange}
  disabled={disabled || models.length === 0}
>
  <Select.Trigger
    size="sm"
    class="max-w-56 min-w-0 text-[13px] border-transparent bg-transparent text-muted-foreground shadow-none hover:bg-muted hover:text-foreground data-[state=open]:bg-muted dark:bg-transparent dark:hover:bg-muted"
    aria-label={`Modèle : ${label}`}
  >
    <span class="truncate">{label}</span>
  </Select.Trigger>
  <Select.Content>
    {#each groups as group (group.provider)}
      <Select.Group>
        <Select.GroupHeading>{group.provider}</Select.GroupHeading>
        {#each group.items as model (model.selection)}
          <Select.Item value={model.selection} label={model.name}>{model.name}</Select.Item>
        {/each}
      </Select.Group>
    {/each}
  </Select.Content>
</Select.Root>
