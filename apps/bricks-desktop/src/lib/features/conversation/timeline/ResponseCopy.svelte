<script lang="ts">
  // The one copy action of a complete response: the Markdown of all its
  // assistant segments (no reasoning, tools, notices or summary).
  import Check from '@lucide/svelte/icons/check'
  import Copy from '@lucide/svelte/icons/copy'

  let { text }: { text: string } = $props()

  let copied = $state(false)
  let copyError = $state<string | null>(null)
  let timer: ReturnType<typeof setTimeout> | undefined
  $effect(() => () => clearTimeout(timer))

  async function copy() {
    try {
      await navigator.clipboard.writeText(text)
      copyError = null
      copied = true
      clearTimeout(timer)
      timer = setTimeout(() => (copied = false), 1500)
    } catch (e) {
      copied = false
      copyError = e instanceof Error ? e.message : String(e)
    }
  }
</script>

<div class="-mt-2 flex items-center gap-2">
  <button
    type="button"
    class="inline-flex size-6 items-center justify-center rounded-md text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
    title="Copier"
    aria-label="Copier"
    onclick={copy}
  >
    {#if copied}
      <Check class="size-3.5" aria-hidden="true" />
    {:else}
      <Copy class="size-3.5" aria-hidden="true" />
    {/if}
  </button>
  {#if copyError}
    <p class="text-xs text-destructive" role="alert">Copie impossible : {copyError}</p>
  {/if}
</div>
