<script lang="ts">
  // A discreet column of grains at the left of the conversation: one per message sent, the one
  // being read darker. A grain scrolls to its message. Only the loaded messages have a grain.
  import { t } from '$lib/i18n/i18n.svelte'

  interface Mark {
    /** The message's key, also its `data-turn` in the list. */
    key: string
    /** The start of the message, for the grain's name. */
    label: string
  }

  let { marks, scroller }: { marks: Mark[]; scroller: HTMLElement | null } = $props()

  let active = $state<string | null>(null)

  function turnOf(key: string): HTMLElement | null {
    return scroller?.querySelector<HTMLElement>(`[data-turn="${CSS.escape(key)}"]`) ?? null
  }

  /** The message being read: the last one whose top is above the upper third of the view. */
  function measure() {
    const el = scroller
    if (!el || marks.length === 0) return
    const line = el.getBoundingClientRect().top + el.clientHeight * 0.35
    let current = marks[0].key
    for (const mark of marks) {
      const turn = turnOf(mark.key)
      if (turn && turn.getBoundingClientRect().top <= line) current = mark.key
    }
    // At the very bottom, the last message is the one read.
    if (el.scrollHeight - el.scrollTop - el.clientHeight < 4) current = marks.at(-1)?.key ?? current
    active = current
  }

  $effect(() => {
    const el = scroller
    void marks.length
    if (!el) return
    let frame = 0
    const onScroll = () => {
      if (frame) return
      frame = requestAnimationFrame(() => {
        frame = 0
        measure()
      })
    }
    measure()
    el.addEventListener('scroll', onScroll, { passive: true })
    return () => {
      el.removeEventListener('scroll', onScroll)
      cancelAnimationFrame(frame)
    }
  })

  function go(key: string) {
    const turn = turnOf(key)
    if (!turn || !scroller) return
    const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches
    turn.scrollIntoView({ block: 'start', behavior: reduced ? 'auto' : 'smooth' })
    active = key
  }

  /** One tab stop for the whole rail: the arrows move between grains, Enter goes. */
  function onKeydown(event: KeyboardEvent, index: number) {
    const next =
      event.key === 'ArrowDown' ? index + 1
      : event.key === 'ArrowUp' ? index - 1
      : event.key === 'Home' ? 0
      : event.key === 'End' ? marks.length - 1
      : null
    if (next === null) return
    event.preventDefault()
    const target = marks[Math.max(0, Math.min(marks.length - 1, next))]
    active = target.key
    const nav = (event.currentTarget as HTMLElement).parentElement
    nav?.querySelector<HTMLElement>(`[data-mark="${CSS.escape(target.key)}"]`)?.focus()
  }

  const focusable = $derived(active ?? marks.at(-1)?.key ?? null)
</script>

{#if marks.length > 1}
  <nav
    aria-label={t('conversation.messageRail')}
    class="pointer-events-none absolute top-1/2 left-1 z-10 hidden max-h-[70%] -translate-y-1/2 flex-col items-center @[54rem]:flex"
  >
    {#each marks as mark, index (mark.key)}
      <button
        type="button"
        data-mark={mark.key}
        tabindex={focusable === mark.key ? 0 : -1}
        class="group pointer-events-auto flex max-h-3.5 min-h-0.5 w-6 flex-1 items-center justify-center focus-visible:outline-none"
        aria-label={mark.label}
        aria-current={active === mark.key ? 'true' : undefined}
        title={mark.label}
        onclick={() => go(mark.key)}
        onkeydown={(event) => onKeydown(event, index)}
      >
        <span
          class={[
            'size-1.5 rounded-full transition-[background-color,transform] duration-150 group-hover:scale-150 group-focus-visible:ring-2 group-focus-visible:ring-ring',
            active === mark.key ? 'bg-foreground/55' : 'bg-muted-foreground/25 group-hover:bg-muted-foreground/60',
          ]}
        ></span>
      </button>
    {/each}
  </nav>
{/if}
