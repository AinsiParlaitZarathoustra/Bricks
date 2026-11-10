<script lang="ts">
  // The welcome's line, typed as on a keyboard: « Bienvenue sur Bricks. » first, then the other
  // lines in a random order, each typed, held, then erased. Keys come at uneven intervals, with a
  // longer pause after punctuation. With reduced motion the lines change whole, without typing.
  import { untrack } from 'svelte'
  import { t, type MessageKey } from '$lib/i18n/i18n.svelte'

  const FIRST: MessageKey = 'welcome.line1'
  const OTHERS: MessageKey[] = [
    'welcome.line2',
    'welcome.line3',
    'welcome.line4',
    'welcome.line5',
    'welcome.line6',
    'welcome.line7',
  ]

  let key = $state<MessageKey>(FIRST)
  /** Characters of the line shown so far. */
  let shown = $state(0)

  const line = $derived(t(key))
  const visible = $derived(line.slice(0, shown))

  /** The other lines, shuffled, never starting with `last`. */
  function shuffled(last: MessageKey | null): MessageKey[] {
    const out = [...OTHERS]
    for (let i = out.length - 1; i > 0; i--) {
      const j = Math.floor(Math.random() * (i + 1))
      ;[out[i], out[j]] = [out[j], out[i]]
    }
    if (out[0] === last) out.push(out.shift()!)
    return out
  }

  /** Delay before the next key: uneven, longer after punctuation. */
  function keyDelay(typed: string): number {
    const base = 35 + Math.random() * 75
    return /[.,;:!?]$/.test(typed) ? base + 220 : base
  }

  $effect(() => {
    const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches
    let timer: ReturnType<typeof setTimeout> | undefined
    let queue: MessageKey[] = []
    let cancelled = false
    const wait = (ms: number) =>
      new Promise<void>((resolve) => {
        timer = setTimeout(resolve, ms)
      })

    async function run() {
      let current: MessageKey = FIRST
      while (!cancelled) {
        key = current
        const text = t(current)
        if (reduced) {
          shown = text.length
          await wait(4000)
        } else {
          for (shown = 0; shown < text.length && !cancelled; ) {
            shown += 1
            await wait(keyDelay(text.slice(0, shown)))
          }
          await wait(2600)
          while (shown > 0 && !cancelled) {
            shown -= 1
            await wait(22)
          }
          await wait(380)
        }
        if (queue.length === 0) queue = shuffled(current)
        current = queue.shift()!
      }
    }
    // The loop drives `key` and `shown`; it must not make this effect depend on them.
    untrack(() => void run())
    return () => {
      cancelled = true
      clearTimeout(timer)
    }
  })
</script>

<p class="mt-1.5 min-h-6 text-base text-muted-foreground">
  <span class="sr-only">{line}</span>
  <span aria-hidden="true">{visible}<span class="caret"></span></span>
</p>

<style>
  .caret {
    display: inline-block;
    width: 1.5px;
    height: 1.05em;
    margin-left: 2px;
    vertical-align: -0.15em;
    background: currentColor;
    animation: blink 1s steps(1) infinite;
  }

  @keyframes blink {
    50% {
      opacity: 0;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .caret {
      animation: none;
    }
  }
</style>
