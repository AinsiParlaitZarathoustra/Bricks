<script lang="ts">
  // The « Skills » choice opened from the `+` menu: the usable skills of this
  // context, filtered by the engine (list_skills in a conversation, the catalogue
  // without one). Choosing a skill only adds it to the draft: no trust is granted
  // here, and the project skills that await the trust decision are not offered.
  import { onMount } from 'svelte'
  import * as Command from '$lib/components/ui/command'
  import { Button } from '$lib/components/ui/button'
  import XIcon from '@lucide/svelte/icons/x'
  import { app } from '$lib/state/app.svelte'
  import type { Conversation } from '$lib/state/conversation.svelte'
  import { errorMessage, skillsCatalog } from '$lib/ipc/client'
  import type { SkillsListing } from '$lib/ipc/engine'
  import { hasProjectSkillsAwaitingApproval, usableSkills } from './skills'

  let {
    conversation,
    projectId,
    selected,
    onToggle,
    onClose,
  }: {
    conversation: Conversation | null
    /** Project whose skills are listed without a conversation (null: user and built-in only). */
    projectId: string | null
    selected: string[]
    onToggle: (name: string) => void
    onClose: () => void
  } = $props()

  let query = $state('')
  let failure = $state<string | null>(null)
  /** Answer of the catalogue (no conversation), for the latest request. */
  let catalogListing = $state<SkillsListing | null>(null)
  /** When the latest request was sent: only an engine answer after it counts. */
  let requestedAt = $state(Date.now())
  let inputEl = $state<HTMLInputElement | null>(null)
  /** Requests in flight: an answer to a superseded request is dropped. */
  let seq = 0
  /** The pause after typing, before the request. */
  let timer: ReturnType<typeof setTimeout> | undefined

  /** The listing of the latest request, or null while it is pending. */
  const listing = $derived.by<SkillsListing | null>(() => {
    if (!conversation) return catalogListing
    // The engine answers `list_skills` with a `skills` event of the conversation.
    const answer = conversation.skillsListing
    return answer && answer.at >= requestedAt ? answer.listing : null
  })
  const pending = $derived(listing === null && failure === null)
  const choices = $derived(listing ? usableSkills(listing.entries) : [])
  const awaitingTrust = $derived(listing ? hasProjectSkillsAwaitingApproval(listing) : false)
  const truncated = $derived(listing ? listing.total > listing.entries.length : false)

  async function request(text: string) {
    const mine = ++seq
    requestedAt = Date.now()
    failure = null
    catalogListing = null
    if (conversation) {
      const error = await app.command(conversation.sessionId, { type: 'list_skills', query: text })
      if (mine === seq && error !== null) failure = error
      return
    }
    try {
      const catalog = await skillsCatalog(projectId, text, 0, false)
      if (mine === seq) catalogListing = catalog.listing
    } catch (e) {
      if (mine === seq) failure = errorMessage(e)
    }
  }

  /** Escape closes the choice (the focus goes back to the editor). */
  function onEscape(event: KeyboardEvent) {
    if (event.key !== 'Escape') return
    event.preventDefault()
    event.stopPropagation()
    onClose()
  }

  /** A short pause after typing, then one request for the text typed. */
  function scheduleRequest(text: string, delay: number) {
    clearTimeout(timer)
    timer = setTimeout(() => void request(text), delay)
  }

  function onQueryInput(event: Event & { currentTarget: HTMLInputElement }) {
    const text = event.currentTarget.value.trim()
    scheduleRequest(text, text === '' ? 0 : 200)
  }

  onMount(() => {
    // The choice takes the keyboard when it opens; the first list is requested now.
    inputEl?.focus()
    scheduleRequest('', 0)
    return () => {
      clearTimeout(timer)
      seq += 1
    }
  })

</script>

<div
  role="group"
  aria-label="Skills pour le prochain message"
  class="absolute inset-x-0 bottom-full z-20 mb-2 overflow-hidden rounded-lg border bg-popover text-popover-foreground shadow-md"
>
  <Command.Root shouldFilter={false} class="bg-transparent p-0">
    <Command.Input
      bind:ref={inputEl}
      bind:value={query}
      placeholder="Rechercher un skill"
      aria-label="Rechercher un skill"
      oninput={onQueryInput}
      onkeydown={onEscape}
    />
    <Command.List class="p-1">
      {#if pending}
        <p role="status" class="px-2 py-3 text-sm text-muted-foreground">Chargement des skills…</p>
      {:else if failure}
        <div role="alert" class="flex flex-col items-start gap-2 px-2 py-3 text-sm text-destructive">
          <p class="break-words">Skills indisponibles : {failure}</p>
          <Button variant="outline" size="sm" onclick={() => void request(query.trim())}>Réessayer</Button>
        </div>
      {:else if choices.length === 0}
        <p role="status" class="px-2 py-3 text-sm text-muted-foreground">
          {query.trim() ? `Aucun skill utilisable ne correspond à « ${query.trim()} ».` : 'Aucun skill utilisable ici.'}
        </p>
      {:else}
        {#each choices as skill (skill.name)}
          <Command.Item
            value={skill.name}
            onSelect={() => onToggle(skill.name)}
            data-checked={selected.includes(skill.name)}
            class="gap-2"
          >
            <span class="flex min-w-0 flex-1 flex-col">
              <span class="truncate font-mono text-sm">{skill.name}</span>
              {#if skill.description}
                <span class="truncate text-xs text-muted-foreground">{skill.description}</span>
              {/if}
            </span>
            {#if skill.active}
              <span class="shrink-0 text-xs text-muted-foreground">Actif</span>
            {/if}
          </Command.Item>
        {/each}
      {/if}
    </Command.List>
  </Command.Root>

  <div class="flex flex-wrap items-center justify-between gap-2 border-t px-2 py-1.5 text-xs text-muted-foreground">
    <div class="flex min-w-0 flex-col">
      {#if awaitingTrust}
        <span class="break-words">Skills du projet en attente d’autorisation : non proposés.</span>
      {/if}
      {#if truncated}
        <span class="break-words">Seuls les premiers résultats sont affichés : affinez la recherche.</span>
      {/if}
    </div>
    <Button variant="ghost" size="sm" onclick={onClose} onkeydown={onEscape}>
      <XIcon aria-hidden="true" />
      Fermer
    </Button>
  </div>
</div>
