<script lang="ts">
  // One action under a message: a ghost icon button named by its tooltip.
  import type { Snippet } from 'svelte'
  import { Button } from '$lib/components/ui/button'
  import * as Tooltip from '$lib/components/ui/tooltip'

  interface Props {
    /** Accessible name and tooltip. */
    label: string
    onclick: () => void
    disabled?: boolean
    ref?: HTMLElement | null
    children: Snippet
  }

  let { label, onclick, disabled = false, ref = $bindable(null), children }: Props = $props()
</script>

<Tooltip.Root>
  <Tooltip.Trigger>
    {#snippet child({ props })}
      <Button {...props} bind:ref variant="ghost" size="icon-xs" aria-label={label} {disabled} {onclick}>
        {@render children()}
      </Button>
    {/snippet}
  </Tooltip.Trigger>
  <Tooltip.Content>{label}</Tooltip.Content>
</Tooltip.Root>
