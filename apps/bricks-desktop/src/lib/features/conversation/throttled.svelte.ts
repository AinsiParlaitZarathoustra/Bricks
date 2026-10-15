// A string that follows its source at most once per `ms` while `active()`
// is true (streaming), and immediately otherwise.

export function throttledValue(source: () => string, active: () => boolean, ms = 80) {
  let value = $state(source())
  let last = 0
  let timer: ReturnType<typeof setTimeout> | undefined

  const flush = () => {
    timer = undefined
    last = Date.now()
    value = source()
  }

  $effect(() => {
    const current = source()
    if (!active()) {
      if (timer !== undefined) clearTimeout(timer)
      timer = undefined
      value = current
      return
    }
    const wait = ms - (Date.now() - last)
    if (wait <= 0) {
      if (timer !== undefined) clearTimeout(timer)
      flush()
    } else if (timer === undefined) {
      timer = setTimeout(flush, wait)
    }
  })

  $effect(() => () => {
    if (timer !== undefined) clearTimeout(timer)
  })

  return {
    get value() {
      return value
    },
  }
}
