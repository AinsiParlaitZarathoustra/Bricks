<script lang="ts">
  import { open } from '@tauri-apps/plugin-dialog'
  import { buttonVariants } from '$lib/components/ui/button'
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu'
  import { errorMessage } from '$lib/ipc/client'
  import PlusIcon from '@lucide/svelte/icons/plus'
  import FileIcon from '@lucide/svelte/icons/file'
  import FolderIcon from '@lucide/svelte/icons/folder'

  let {
    disabled = false,
    onPick,
    onError,
  }: {
    disabled?: boolean
    onPick: (paths: string[], kind: 'file' | 'folder') => void
    onError: (message: string) => void
  } = $props()

  /** Native picker (several items at once); the paths are real file-system paths. */
  async function pick(kind: 'file' | 'folder') {
    try {
      const folder = kind === 'folder'
      const picked = await open({
        multiple: true,
        directory: folder,
        title: folder ? 'Choisir des dossiers' : 'Choisir des fichiers',
      })
      const paths = picked === null ? [] : Array.isArray(picked) ? picked : [picked]
      if (paths.length > 0) onPick(paths, kind)
    } catch (e) {
      onError(errorMessage(e))
    }
  }
</script>

<DropdownMenu.Root>
  <DropdownMenu.Trigger
    class={buttonVariants({ variant: 'ghost', size: 'icon-sm' })}
    {disabled}
    aria-label="Joindre un fichier ou un dossier"
    title="Joindre"
  >
    <PlusIcon aria-hidden="true" />
  </DropdownMenu.Trigger>
  <DropdownMenu.Content align="start" side="top">
    <DropdownMenu.Item onSelect={() => pick('file')}>
      <FileIcon aria-hidden="true" />
      Fichiers…
    </DropdownMenu.Item>
    <DropdownMenu.Item onSelect={() => pick('folder')}>
      <FolderIcon aria-hidden="true" />
      Dossier…
    </DropdownMenu.Item>
  </DropdownMenu.Content>
</DropdownMenu.Root>
