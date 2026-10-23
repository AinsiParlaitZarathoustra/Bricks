<script lang="ts">
  import { GENERIC_ICON_ID, iconId, iconUrl, type IconKind } from './resolve'

  let {
    path = null,
    kind = 'file',
    open = false,
    language = null,
  }: { path?: string | null; kind?: IconKind; open?: boolean; language?: string | null } = $props()

  let id = $derived(iconId({ path, kind, open, language }))
  /** An image that failed: its own id, then the generic one; never retried in a loop. */
  let failedId = $state<string | null>(null)
  let genericBroken = $state(false)
  let shown = $derived(id !== failedId ? id : genericBroken ? null : GENERIC_ICON_ID)

  function onerror() {
    if (shown === GENERIC_ICON_ID) genericBroken = true
    failedId = id
  }
</script>

{#if shown}
  <!-- Decorative: the name next to it says what it is. -->
  <img
    class="file-icon"
    src={iconUrl(shown)}
    alt=""
    aria-hidden="true"
    width="16"
    height="16"
    draggable="false"
    decoding="async"
    {onerror}
  />
{/if}
