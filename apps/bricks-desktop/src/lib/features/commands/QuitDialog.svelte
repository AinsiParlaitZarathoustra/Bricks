<script lang="ts">
  // Confirmation of `/quit` while a run is active: it lists the conversations
  // with a run in progress, then closes the application through the native
  // command.
  import * as AlertDialog from '$lib/components/ui/alert-dialog'
  import { appQuit } from '$lib/ipc/client'
  import { app } from '$lib/state/app.svelte'

  let {
    runs,
    onClose,
  }: {
    /** Titles of the conversations with a run in progress; `null` when closed. */
    runs: string[] | null
    onClose: () => void
  } = $props()

  async function quit() {
    try {
      await appQuit()
    } catch (e) {
      app.fail(e)
      onClose()
    }
  }
</script>

<AlertDialog.Root
  open={runs !== null}
  onOpenChange={(open) => {
    if (!open) onClose()
  }}
>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>Quitter Bricks ?</AlertDialog.Title>
      <AlertDialog.Description>
        Une génération est en cours dans les conversations suivantes :
      </AlertDialog.Description>
    </AlertDialog.Header>
    <ul class="list-disc pl-5 text-sm">
      {#each runs ?? [] as title, i (i)}
        <li>{title}</li>
      {/each}
    </ul>
    <AlertDialog.Footer>
      <AlertDialog.Cancel>Rester</AlertDialog.Cancel>
      <AlertDialog.Action onclick={quit}>Quitter</AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
