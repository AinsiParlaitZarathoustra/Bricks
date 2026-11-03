<script lang="ts">
  // Read-only window of a project file opened from a file reference in a
  // message. The path is resolved and checked by Rust (`project_file_read`)
  // in the project of the conversation the message belongs to.
  import { t } from '$lib/i18n/i18n.svelte'
  import { tick } from 'svelte'
  import FileIcon from '$lib/features/file-icons/FileIcon.svelte'
  import { projectFileRead } from '$lib/ipc/client'
  import type { ProjectFileWindow } from '$lib/features/conversation/markdown/types'

  let { args }: { args: Record<string, string | boolean | undefined> } = $props()

  const projectId = $derived(typeof args.projectId === 'string' ? args.projectId : '')
  const path = $derived(typeof args.path === 'string' ? args.path : '')
  const line = $derived(typeof args.line === 'string' ? Number(args.line) : undefined)
  const endLine = $derived(typeof args.endLine === 'string' ? Number(args.endLine) : undefined)

  let file = $state<ProjectFileWindow | null>(null)
  let error = $state<string | null>(null)
  let scroller = $state<HTMLDivElement | null>(null)

  $effect(() => {
    const request = { projectId, path, line, endLine }
    let cancelled = false
    file = null
    error = null
    projectFileRead(request.projectId, request.path, request.line, request.endLine).then(
      async (result) => {
        if (cancelled) return
        file = result
        await tick()
        scroller?.querySelector<HTMLElement>('[data-first-marked]')?.scrollIntoView({ block: 'center' })
      },
      (e: unknown) => {
        if (!cancelled) error = e instanceof Error ? e.message : String(e)
      },
    )
    return () => {
      cancelled = true
    }
  })

  const first = $derived(line ?? 0)
  const last = $derived(endLine ?? line ?? 0)
</script>

<div class="flex min-h-0 flex-col gap-2 text-ui">
  <p class="flex min-w-0 items-start gap-1.5 font-mono text-xs text-muted-foreground">
    <span class="flex h-4 shrink-0 items-center"><FileIcon {path} /></span>
    <span class="min-w-0 break-all">{path}{#if line}:{line}{#if endLine && endLine !== line}-{endLine}{/if}{/if}</span>
  </p>
  {#if error}
    <p class="text-xs text-destructive" role="alert">{error}</p>
  {:else if !file}
    <p class="text-xs text-muted-foreground" role="status">{t('common.reading2')}</p>
  {:else}
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <div
      bind:this={scroller}
      class="max-h-[60vh] overflow-auto rounded-md bg-code-bg font-mono text-xs leading-5 text-code-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring"
      role="region"
      aria-label={t('common.contentOf', { path })}
      tabindex="0"
    >
      <div class="min-w-max py-1">
        {#each file.lines as text, i (i)}
          {@const n = file.start + i}
          {@const marked = first > 0 && n >= first && n <= last}
          <div
            class={['grid grid-cols-[3.5rem_minmax(0,1fr)]', marked && 'bg-info-soft']}
            data-first-marked={marked && n === first ? '' : undefined}
          >
            <span class="px-2 text-right text-muted-foreground tabular-nums select-none">{n}</span>
            <span class="px-2 whitespace-pre">{text}</span>
          </div>
        {/each}
      </div>
    </div>
    {#if file.truncated}
      <p class="text-xs text-muted-foreground">
        {t('inspect.linesToOf', { start: file.start, value: file.start + file.lines.length - 1, totalLines: file.totalLines })}
      </p>
    {/if}
  {/if}
</div>
