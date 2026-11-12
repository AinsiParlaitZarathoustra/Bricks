<script lang="ts">
  import { t } from '$lib/i18n/i18n.svelte'
  import { Pencil } from '@lucide/svelte'
  import { Button } from '$lib/components/ui/button'
  import { Input } from '$lib/components/ui/input'
  import * as Breadcrumb from '$lib/components/ui/breadcrumb'
  import { cn } from '$lib/utils.js'
  import StatusDot from '$lib/layout/StatusDot.svelte'
  import { app } from '$lib/state/app.svelte'
  import type { Conversation } from '$lib/state/conversation.svelte'
  import { ui } from '$lib/state/ui.svelte'
  import { agentName } from '$lib/features/agents/branch'

  let { conversation }: { conversation: Conversation | null } = $props()

  /** A link of the trail: a button that looks like the crumb, with a hover surface. */
  const CRUMB =
    'min-w-0 truncate rounded-md px-1.5 py-0.5 hover:bg-element-hover focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none'

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
  « Project › Conversation » (› Sub-agent). The project opens a new conversation in it; the
  conversation's name is where you are (double-click or the pencil to rename it). In a sub-agent's
  branch, the conversation's name leads back to it.
-->
<div class="group/title flex min-w-0 max-w-full items-center justify-center gap-1">
  <Breadcrumb.Root class="min-w-0" aria-label={t('conversation.breadcrumb')}>
    <Breadcrumb.List class="flex-nowrap gap-1 text-ui">
      {#if projectName && project}
        <Breadcrumb.Item class="min-w-0 max-w-[40%] shrink">
          <Breadcrumb.Link>
            {#snippet child({ props })}
              <button
                type="button"
                data-slot="breadcrumb-link"
                class={cn(props.class, CRUMB)}
                title={t('conversation.newConversationIn', { project: projectName })}
                onclick={() => void app.newConversation(project.id)}
              >{projectName}</button>
            {/snippet}
          </Breadcrumb.Link>
        </Breadcrumb.Item>
        <Breadcrumb.Separator />
      {/if}

      {#if branch}
        <Breadcrumb.Item class="min-w-0 shrink">
          <Breadcrumb.Link>
            {#snippet child({ props })}
              <button type="button" data-slot="breadcrumb-link" class={cn(props.class, CRUMB)} {title} onclick={() => ui.closeAgent()}>{title}</button>
            {/snippet}
          </Breadcrumb.Link>
        </Breadcrumb.Item>
        <Breadcrumb.Separator />
      {/if}

      <!-- The page's h1 is the sr-only « Bricks » (AppShell): where you are is its h2. -->
      <Breadcrumb.Item class="min-w-0">
        <h2 class="flex min-w-0 items-center font-medium">
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
            <Breadcrumb.Page class="min-w-0 truncate px-1.5 py-0.5" data-tauri-drag-region>{agentName(branch)}</Breadcrumb.Page>
          {:else}
            <Breadcrumb.Page class="min-w-0 truncate px-1.5 py-0.5" {title} ondblclick={startEdit}>{title}</Breadcrumb.Page>
          {/if}
        </h2>
      </Breadcrumb.Item>
    </Breadcrumb.List>
  </Breadcrumb.Root>

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
