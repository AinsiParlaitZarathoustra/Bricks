<script lang="ts">
  // The actions of a complete response: rate it (« Bonne réponse », « Mauvaise
  // réponse », one at a time) and copy its Markdown (all its assistant segments,
  // no reasoning, tools, notices or summary).
  import { t } from '$lib/i18n/i18n.svelte'
  import Check from '@lucide/svelte/icons/check'
  import Copy from '@lucide/svelte/icons/copy'
  import ThumbsDown from '@lucide/svelte/icons/thumbs-down'
  import ThumbsUp from '@lucide/svelte/icons/thumbs-up'
  import * as Message from '$lib/components/ui/message'
  import { Toggle } from '$lib/components/ui/toggle'
  import * as Tooltip from '$lib/components/ui/tooltip'
  import { ratings, type Rating } from '../feedback.svelte'
  import MessageAction from './MessageAction.svelte'

  let { text, sessionId }: { text: string; sessionId: string } = $props()

  /** A rating belongs to this exact answer: its conversation and its text. */
  function textHash(value: string): string {
    let h = 5381
    for (let i = 0; i < value.length; i++) h = ((h << 5) + h + value.charCodeAt(i)) | 0
    return (h >>> 0).toString(36) + value.length.toString(36)
  }
  const key = $derived(ratings.key(sessionId, textHash(text)))
  const rating = $derived(ratings.get(key))

  const choices = $derived([
    { value: 'good' as Rating, label: t('conversation.goodResponse'), Icon: ThumbsUp },
    { value: 'bad' as Rating, label: t('conversation.badResponse'), Icon: ThumbsDown },
  ])

  function rate(value: Rating, pressed: boolean) {
    ratings.set(key, pressed ? value : null)
  }

  let copied = $state(false)
  let copyError = $state<string | null>(null)
  let timer: ReturnType<typeof setTimeout> | undefined
  $effect(() => () => clearTimeout(timer))

  async function copy() {
    try {
      await navigator.clipboard.writeText(text)
      copyError = null
      copied = true
      clearTimeout(timer)
      timer = setTimeout(() => (copied = false), 2000)
    } catch (e) {
      copied = false
      copyError = e instanceof Error ? e.message : String(e)
    }
  }
</script>

<Tooltip.Provider delayDuration={400}>
  <Message.Footer class="-mt-2 gap-0.5 px-0">
    {#each choices as { value, label, Icon } (value)}
      <Tooltip.Root>
        <Tooltip.Trigger>
          {#snippet child({ props })}
            <Toggle
              {...props}
              size="icon-xs"
              aria-label={label}
              bind:pressed={() => rating === value, (pressed) => rate(value, pressed)}
            >
              <Icon aria-hidden="true" />
            </Toggle>
          {/snippet}
        </Tooltip.Trigger>
        <Tooltip.Content>{label}</Tooltip.Content>
      </Tooltip.Root>
    {/each}
    <MessageAction label={t('common.copy')} onclick={copy}>
      {#if copied}
        <Check aria-hidden="true" />
      {:else}
        <Copy aria-hidden="true" />
      {/if}
    </MessageAction>
    {#if copyError}
      <p class="ml-2 font-normal text-destructive" role="alert">{t('conversation.couldNotCopy', { copyError })}</p>
    {/if}
  </Message.Footer>
</Tooltip.Provider>
