<script lang="ts">
  // One applied edit operation: a compact line per file (name and the
  // operation's counts). The full path is the name's tooltip; a click opens
  // the file in the reader.
  import { t } from '$lib/i18n/i18n.svelte'
  import DiffStat from '$lib/layout/DiffStat.svelte'
  import FileIcon from '$lib/features/file-icons/FileIcon.svelte'
  import type { Conversation, EditItem } from '$lib/state/conversation.svelte'
  import { reader } from '$lib/state/reader.svelte'
  import { CHANGE_KIND_LABEL } from '../format'

  let { item, conversation, inline = false }: { item: EditItem; conversation: Conversation; inline?: boolean } = $props()

  const agentName = $derived.by(() => {
    if (!item.agentId) return null
    return conversation.agents.get(item.agentId)?.spawn.profile ?? t('conversation.unknownAgent', { value: item.agentId.slice(0, 8) })
  })

  function baseName(path: string): string {
    return path.split(/[\\/]/).filter(Boolean).pop() ?? path
  }

  function open(file: EditItem['files'][number]) {
    reader.open({
      sessionId: conversation.sessionId,
      projectId: conversation.projectId,
      agentId: item.agentId,
      path: file.path,
      absolute: file.absolute ?? null,
    })
  }
</script>

<ul
  aria-label={agentName ? t('conversation.filesChangedBy', { agentName }) : t('conversation.filesChanged')}
  class={inline ? 'contents' : 'flex flex-col gap-0.5 py-0.5 text-ui'}
>
  {#each item.files as file, i (`${file.absolute ?? file.path}#${i}`)}
    <li class="flex min-w-0 items-center gap-2">
      <button
        type="button"
        class="inline-flex min-w-0 items-center gap-1.5 rounded-sm text-start font-mono text-xs hover:underline focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
        title={`${file.absolute ?? file.path} · ${CHANGE_KIND_LABEL[file.kind]}${agentName ? ` · ${agentName}` : ''}`}
        onclick={() => open(file)}
      >
        <FileIcon path={file.path} />
        <span class="min-w-0 truncate">{baseName(file.path)}</span>
      </button>
      {#if file.binary}
        <span class="text-xs text-muted-foreground">{t('common.binary')}</span>
      {:else}
        <DiffStat additions={file.added} deletions={file.removed} />
      {/if}
    </li>
  {/each}
</ul>
