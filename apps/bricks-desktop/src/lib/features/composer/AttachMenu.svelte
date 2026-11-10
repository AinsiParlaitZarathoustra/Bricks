<script lang="ts">
  import { cn } from '$lib/utils'
  import { t } from '$lib/i18n/i18n.svelte'
  import { open } from '@tauri-apps/plugin-dialog'
  import { buttonVariants } from '$lib/components/ui/button'
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu'
  import { errorMessage } from '$lib/ipc/client'
  import PlusIcon from '@lucide/svelte/icons/plus'
  import FileIcon from '@lucide/svelte/icons/file'
  import FolderIcon from '@lucide/svelte/icons/folder'
  import SparklesIcon from '@lucide/svelte/icons/sparkles'

  let {
    disabled = false,
    onPick,
    onError,
    onSkills,
  }: {
    disabled?: boolean
    onPick: (paths: string[], kind: 'file' | 'folder') => void
    onError: (message: string) => void
    /** Opens the skills choice of the composer (the menu entry is shown only when given). */
    onSkills?: () => void
  } = $props()

  /** Set when the skills choice was asked: the menu must not give the focus back to its trigger. */
  let skillsAsked = false

  /** Native picker (several items at once); the paths are real file-system paths. */
  async function pick(kind: 'file' | 'folder') {
    try {
      const folder = kind === 'folder'
      const picked = await open({
        multiple: true,
        directory: folder,
        title: folder ? t('composer.chooseFolders') : t('composer.chooseFiles'),
      })
      const paths = picked === null ? [] : Array.isArray(picked) ? picked : [picked]
      if (paths.length > 0) onPick(paths, kind)
    } catch (e) {
      onError(errorMessage(e))
    }
  }

  function askSkills() {
    skillsAsked = true
    onSkills?.()
  }
</script>

<DropdownMenu.Root>
  <DropdownMenu.Trigger
    class={cn(
      buttonVariants({ variant: 'ghost', size: 'icon-compact' }),
      'rounded-full border border-border bg-card text-muted-foreground hover:bg-element-hover hover:text-foreground data-[state=open]:border-border-strong data-[state=open]:bg-element-selected',
    )}
    {disabled}
    aria-label={t('composer.attachAFileOrFolder')}
    title={t('composer.attach')}
  >
    <PlusIcon aria-hidden="true" />
  </DropdownMenu.Trigger>
  <DropdownMenu.Content
    align="start"
    side="top"
    onCloseAutoFocus={(event) => {
      if (!skillsAsked) return
      // The skills choice takes the focus it opens with.
      skillsAsked = false
      event.preventDefault()
    }}
  >
    <DropdownMenu.Item onSelect={() => pick('file')}>
      <FileIcon aria-hidden="true" />
      {t('composer.files')}
    </DropdownMenu.Item>
    <DropdownMenu.Item onSelect={() => pick('folder')}>
      <FolderIcon aria-hidden="true" />
      {t('composer.folder')}
    </DropdownMenu.Item>
    {#if onSkills}
      <DropdownMenu.Item onSelect={askSkills}>
        <SparklesIcon aria-hidden="true" />
        {t('composer.skills')}
      </DropdownMenu.Item>
    {/if}
  </DropdownMenu.Content>
</DropdownMenu.Root>
