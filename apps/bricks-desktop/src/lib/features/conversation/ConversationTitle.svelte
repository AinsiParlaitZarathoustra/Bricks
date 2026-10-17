<script lang="ts">
  import { Pencil } from '@lucide/svelte'
  import { Button } from '$lib/components/ui/button'
  import { Input } from '$lib/components/ui/input'
  import StatusDot from '$lib/layout/StatusDot.svelte'
  import { app } from '$lib/state/app.svelte'
  import type { Conversation } from '$lib/state/conversation.svelte'

  let { conversation }: { conversation: Conversation | null } = $props()

  let editing = $state(false)
  let draft = $state('')
  let inputEl = $state<HTMLInputElement | null>(null)

  const projectName = $derived(
    (conversation && app.projects.find((p) => p.id === conversation.projectId)?.name) ??
      app.currentProject?.name ??
      null,
  )
  const title = $derived(conversation ? conversation.title.trim() || 'Nouvelle conversation' : 'Bricks')

  $effect(() => {
    if (editing && inputEl) {
      inputEl.focus()
      inputEl.select()
    }
  })

  function startEdit() {
    if (!conversation) return
    draft = conversation.title
    editing = true
  }

  function cancelEdit() {
    editing = false
  }

  /** Saves the title once; the input leaving the DOM also triggers blur, which is ignored here. */
  function commit() {
    if (!editing || !conversation) return
    editing = false
    const next = draft.trim()
    if (!next || next === conversation.title) return
    void app.renameSession(conversation.sessionId, next)
  }

  function onKeydown(event: KeyboardEvent) {
    if (event.key === 'Enter') {
      event.preventDefault()
      commit()
    } else if (event.key === 'Escape') {
      event.preventDefault()
      cancelEdit()
    }
  }
</script>

<div class="flex min-w-0 max-w-full items-center justify-center gap-2">
  <!-- The page's h1 is the sr-only « Bricks » (AppShell): the conversation title is its h2. -->
  <h2 class="flex min-w-0 items-center text-sm font-medium text-foreground">
    {#if editing && conversation}
      <Input
        bind:ref={inputEl}
        bind:value={draft}
        class="h-7 w-64 max-w-full text-sm"
        aria-label="Titre de la conversation"
        maxlength={200}
        onkeydown={onKeydown}
        onblur={commit}
      />
    {:else}
      <!-- Plain text: renaming is the pencil button beside it (the single rename control). -->
      <span class="min-w-0 truncate px-1.5 py-0.5" title={title} data-tauri-drag-region>{title}</span>
    {/if}
  </h2>

  {#if conversation && !editing}
    <Button variant="ghost" size="icon-xs" aria-label="Modifier le titre" onclick={startEdit}>
      <Pencil aria-hidden="true" />
    </Button>
  {/if}

  {#if projectName}
    <span class="min-w-0 truncate text-xs text-muted-foreground" data-tauri-drag-region>{projectName}</span>
  {/if}

  {#if conversation?.running}
    <StatusDot status="running" label="En cours" />
  {/if}
</div>
