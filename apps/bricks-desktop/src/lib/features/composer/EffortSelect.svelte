<script lang="ts">
  import * as Select from '$lib/components/ui/select'
  import type { ProfileChoice } from '$lib/ipc/engine'

  /** Sentinel value of the « Par défaut » option (it maps to `null`). */
  const DEFAULT_VALUE = '__default__'

  let {
    open = $bindable(false),
    value,
    effective,
    profiles,
    defaultProfile,
    disabled = false,
    onChange,
  }: {
    /** Opened from outside (`/effort`); the select reports its own state. */
    open?: boolean
    /** Requested profile id, `null` for the model's default. */
    value: string | null
    /** Profile id the engine actually applies. */
    effective: string | null
    profiles: ProfileChoice[]
    defaultProfile: string | null
    disabled?: boolean
    /** Resolves with the refusal message, or `null` when the change is accepted. */
    onChange: (profile: string | null) => Promise<string | null>
  } = $props()

  /** Shown choice: follows `value`, shows the pick at once, reverts if refused. */
  let shown = $derived(value ?? DEFAULT_VALUE)

  function labelOf(id: string | null | undefined): string {
    if (!id) return ''
    return profiles.find((p) => p.id === id)?.label ?? id
  }

  function defaultText(id: string | null): string {
    const name = labelOf(id)
    return name ? `Par défaut (${name})` : 'Par défaut'
  }

  const triggerLabel = $derived(
    shown === DEFAULT_VALUE ? defaultText(effective ?? defaultProfile) : labelOf(shown),
  )

  async function handleChange(next: string | undefined) {
    if (!next || next === shown) return
    const requested = next === DEFAULT_VALUE ? null : next
    shown = next
    const refusal = await onChange(requested)
    if (refusal) shown = value ?? DEFAULT_VALUE
  }
</script>

<Select.Root bind:open type="single" value={shown} onValueChange={handleChange} {disabled}>
  <Select.Trigger
    size="sm"
    class="max-w-56 min-w-0 text-[13px] border-transparent bg-transparent text-muted-foreground shadow-none hover:bg-muted hover:text-foreground data-[state=open]:bg-muted dark:bg-transparent dark:hover:bg-muted"
    aria-label={`Effort de raisonnement : ${triggerLabel}`}
  >
    <span class="truncate">{triggerLabel}</span>
  </Select.Trigger>
  <Select.Content>
    <Select.Item value={DEFAULT_VALUE} label={defaultText(defaultProfile)}>{defaultText(defaultProfile)}</Select.Item>
    {#each profiles as profile (profile.id)}
      <Select.Item value={profile.id} label={profile.label}>{profile.label}</Select.Item>
    {/each}
  </Select.Content>
</Select.Root>
