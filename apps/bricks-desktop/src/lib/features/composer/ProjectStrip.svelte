<script lang="ts">
  import { untrack } from 'svelte'
  import { app } from '$lib/state/app.svelte'
  import { errorMessage, gitStatus } from '$lib/ipc/client'
  import FolderIcon from '@lucide/svelte/icons/folder'
  import GitBranchIcon from '@lucide/svelte/icons/git-branch'
  import TriangleAlertIcon from '@lucide/svelte/icons/triangle-alert'

  type GitView = {
    projectId: string
    status: Awaited<ReturnType<typeof gitStatus>> | null
    error: string | null
  }

  /** What the strip shows, computed from the project and the last Git read. */
  type Shown =
    | { kind: 'idle' }
    | { kind: 'missing' }
    | { kind: 'loading' }
    | { kind: 'error'; message: string }
    | { kind: 'none' }
    | { kind: 'detached'; short: string }
    | { kind: 'branch'; name: string }

  let view = $state<GitView | null>(null)
  let loadSeq = 0
  let lastFocusRefresh = 0

  /** Reads the real Git state of the current project. Stale answers are dropped. */
  async function refresh() {
    const project = app.currentProject
    if (!project || project.missing || project.chats) {
      view = null
      return
    }
    const seq = ++loadSeq
    const projectId = project.id
    try {
      const status = await gitStatus(projectId)
      if (seq === loadSeq) view = { projectId, status, error: null }
    } catch (e) {
      if (seq === loadSeq) view = { projectId, status: null, error: errorMessage(e) }
    }
  }

  // Re-read when the project changes, and when a session of it starts or
  // finishes a run (the engine's run events refresh `app.sessions`).
  $effect(() => {
    void app.currentProjectId
    void app.sessions
    untrack(() => void refresh())
  })

  function onWindowFocus() {
    const now = Date.now()
    if (now - lastFocusRefresh < 2000) return
    lastFocusRefresh = now
    void refresh()
  }

  const project = $derived(app.currentProject)

  const shown = $derived.by((): Shown => {
    const p = project
    if (!p) return { kind: 'idle' }
    if (p.missing) return { kind: 'missing' }
    if (!view || view.projectId !== p.id) return { kind: 'loading' }
    if (view.error) return { kind: 'error', message: view.error }
    const s = view.status
    if (!s || !s.is_repo) return { kind: 'none' }
    if (s.detached) return { kind: 'detached', short: (s.head ?? '').slice(0, 7) || 'inconnu' }
    return { kind: 'branch', name: s.branch ?? 'branche inconnue' }
  })
</script>

<svelte:window onfocus={onWindowFocus} />

<!-- A conversation without a project has no folder or Git state to show. -->
{#if !project?.chats}
<div class="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 text-[13px]">
  {#if shown.kind === 'idle'}
    <p class="text-muted-foreground">Aucun projet sélectionné : créez-en un avec « Nouveau projet ».</p>
  {:else if project}
    <span class="flex min-w-0 max-w-full items-center gap-1.5 font-medium text-foreground" title={project.path}>
      <FolderIcon class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
      <span class="truncate">{project.name}</span>
    </span>

    <span class="text-muted-foreground" aria-hidden="true">·</span>

    <p class="flex min-w-0 items-center gap-1.5 text-muted-foreground" aria-live="polite">
      {#if shown.kind === 'missing'}
        <TriangleAlertIcon class="size-4 shrink-0 text-warning" aria-hidden="true" />
        <span>Dossier introuvable</span>
      {:else if shown.kind === 'loading'}
        <span>Lecture de Git…</span>
      {:else if shown.kind === 'error'}
        <TriangleAlertIcon class="size-4 shrink-0 text-warning" aria-hidden="true" />
        <span class="truncate" title={shown.message}>Git indisponible</span>
      {:else if shown.kind === 'none'}
        <span>Hors dépôt Git</span>
      {:else if shown.kind === 'detached'}
        <GitBranchIcon class="size-4 shrink-0" aria-hidden="true" />
        <span class="truncate font-mono">HEAD détachée ({shown.short})</span>
      {:else if shown.kind === 'branch'}
        <GitBranchIcon class="size-4 shrink-0" aria-hidden="true" />
        <span class="truncate font-mono">{shown.name}</span>
      {/if}
    </p>
  {/if}
</div>
{/if}
