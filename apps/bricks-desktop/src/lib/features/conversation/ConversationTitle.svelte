<script lang="ts">
  import { t } from '$lib/i18n/i18n.svelte'
  import { Pencil } from '@lucide/svelte'
  import { Button } from '$lib/components/ui/button'
  import { Input } from '$lib/components/ui/input'
  import StatusDot from '$lib/layout/StatusDot.svelte'
  import { app } from '$lib/state/app.svelte'
  import type { Conversation } from '$lib/state/conversation.svelte'
  import { ui } from '$lib/state/ui.svelte'
  import { agentName } from '$lib/features/agents/branch'

  let { conversation }: { conversation: Conversation | null } = $props()

  let editing = $state(false)
  let draft = $state('')
  let inputEl = $state<HTMLInputElement | null>(null)

  // A conversation without a project shows no project name.
  const project = $derived(
    (conversation && app.projects.find((p) => p.id === conversation.projectId)) ?? app.currentProject,
  )
  const projectName = $derived(project && !project.chats ? project.name : null)
  const title = $derived(conversation ? conversation.title.trim() || t('common.newConversation') : 'Bricks')
  /** The sub-agent whose branch is shown, after the conversation's title. */
  const branch = $derived(ui.agentId ? (conversation?.agents.get(ui.agentId) ?? null) : null)

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

<!--
  « Project / Conversation ». The project opens a new conversation in it; the conversation's name
  is where you already are (double-click or the pencil to rename it).
-->
<div class="group/title flex min-w-0 max-w-full items-center justify-center gap-1">
  {#if projectName && project}
    <button
      type="button"
      class="min-w-0 max-w-[40%] shrink truncate rounded-md px-1.5 py-0.5 text-ui text-muted-foreground transition-colors hover:bg-element-hover hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
      title={t('conversation.newConversationIn', { project: projectName })}
      onclick={() => void app.newConversation(project.id)}>{projectName}</button
    >
    <span aria-hidden="true" class="text-muted-foreground/60">/</span>
  {/if}

  <!-- The page's h1 is the sr-only « Bricks » (AppShell): the conversation title is its h2. -->
  <h2 class="flex min-w-0 items-center gap-1 text-ui font-medium text-foreground">
    {#if editing && conversation}
      <Input
        bind:ref={inputEl}
        bind:value={draft}
        class="h-7 w-64 max-w-full text-ui"
        aria-label={t('conversation.conversationTitle')}
        maxlength={200}
        onkeydown={onKeydown}
        onblur={commit}
      />
    {:else if branch}
      <button
        type="button"
        class="min-w-0 truncate rounded-md px-1.5 py-0.5 text-muted-foreground hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
        {title}
        onclick={() => ui.closeAgent()}>{title}</button
      >
      <span aria-hidden="true" class="text-muted-foreground/60">/</span>
      <span class="min-w-0 truncate px-1.5 py-0.5" data-tauri-drag-region>{agentName(branch)}</span>
    {:else}
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <span class="min-w-0 truncate px-1.5 py-0.5" {title} ondblclick={startEdit}>{title}</span>
    {/if}
  </h2>

  {#if conversation && !editing && !branch}
    <Button
      variant="ghost"
      size="icon-xs"
      class="opacity-0 transition-opacity group-hover/title:opacity-100 focus-visible:opacity-100"
      aria-label={t('conversation.editTheTitle')}
      onclick={startEdit}
    >
      <Pencil aria-hidden="true" />
    </Button>
  {/if}

  {#if branch ? !branch.endedAt : conversation?.running}
    <StatusDot status="running" label={t('common.running')} />
  {/if}
</div>
