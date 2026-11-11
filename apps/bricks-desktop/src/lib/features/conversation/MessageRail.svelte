<script lang="ts">
  // A discreet column of grains at the left of the conversation: one per message sent, the one
  // being read darker. A grain scrolls to its message; hovered or focused, it shows a card with
  // the message, the start of its answer, its place and its time. Only the loaded messages have
  // a grain.
  import { dateTimeFormat, t } from '$lib/i18n/i18n.svelte'

  interface Mark {
    /** The message's key, also its `data-turn` in the list. */
    key: string
    /** The start of the message, for the grain's name. */
    label: string
    /** The answer's text (empty while it runs, or when there is none). */
    answer: string
    /** When the message was sent, in milliseconds. */
    at: number | null
    /** Its place among all the messages sent in the conversation (from 1), and their number. */
    place: number
    total: number
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

  const when = $derived(dateTimeFormat({ dateStyle: 'long', timeStyle: 'short' }))

  /** The answer as plain text: Markdown's marks go, every other character stays. */
  function plain(text: string): string {
    return text
      .replace(/```[\s\S]*?```/g, ' ')
      .replace(/!\[([^\]]*)\]\([^)]*\)/g, '$1')
      .replace(/\[([^\]]*)\]\([^)]*\)/g, '$1')
      .replace(/`([^`]*)`/g, '$1')
      .replace(/(\*\*|__)(?=\S)([\s\S]*?\S)\1/g, '$2')
      .replace(/(^|[^\w*])\*(?=\S)([^*\n]*?\S)\*(?![\w*])/g, '$1$2')
      .replace(/(^|[^\w])_(?=\S)([^_\n]*?\S)_(?!\w)/g, '$1$2')
      .replace(/^[ \t]*(?:#{1,6}[ \t]+|>[ \t]?|[-*+][ \t]+|\d+[.)][ \t]+)/gm, '')
      .replace(/\s+/g, ' ')
      .trim()
  }

  function position(mark: Mark): string {
    return t('conversation.railPosition', { index: mark.place, total: mark.total })
  }
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
        class="group pointer-events-auto relative flex h-3.5 min-h-1 w-7 shrink items-center justify-center focus-visible:outline-none"
        aria-label={`${mark.label} · ${position(mark)}`}
        aria-current={active === mark.key ? 'true' : undefined}
        onclick={() => go(mark.key)}
        onkeydown={(event) => onKeydown(event, index)}
      >
        <span
          class={[
            'h-1.5 w-3 rounded-full transition-[background-color,transform] duration-150 group-hover:scale-125 group-focus-visible:ring-2 group-focus-visible:ring-ring',
            active === mark.key ? 'bg-foreground/55' : 'bg-muted-foreground/25 group-hover:bg-muted-foreground/60',
          ]}
        ></span>
        <span
          aria-hidden="true"
          class="pointer-events-none invisible absolute top-1/2 left-8 z-20 flex w-72 -translate-y-1/2 flex-col gap-1.5 rounded-2xl border border-border-subtle bg-popover px-4 py-3 text-left text-popover-foreground opacity-0 shadow-float transition-opacity duration-100 group-hover:visible group-hover:opacity-100 group-focus-visible:visible group-focus-visible:opacity-100"
        >
          <span class="truncate text-ui font-medium">{mark.label}</span>
          {#if mark.answer}
            <span class="line-clamp-3 text-compact text-muted-foreground">{plain(mark.answer)}</span>
          {/if}
          <span class="mt-0.5 flex items-center justify-between gap-3 text-caption text-muted-foreground tabular-nums">
            <span>{position(mark)}</span>
            {#if mark.at}
              <span class="truncate">{when.format(new Date(mark.at))}</span>
            {/if}
          </span>
        </span>
      </button>
    {/each}
  </nav>
{/if}
