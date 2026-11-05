// One shared second hand for every live clock on screen (a work session's
// turn and step timers, on the ticket and on every board card). One interval
// for the page rather than one per card, running only while something reads it.
let now = $state(Date.now())
let readers = 0
let timer: ReturnType<typeof setInterval> | undefined

/** Call from a component's script: `const clock = useClock()`, then read
 *  `clock.now`. The interval stops when the last reader unmounts. */
export function useClock(): { readonly now: number } {
  $effect(() => {
    readers += 1
    if (!timer) {
      now = Date.now()
      timer = setInterval(() => (now = Date.now()), 1000)
    }
    return () => {
      readers -= 1
      if (readers === 0 && timer) {
        clearInterval(timer)
        timer = undefined
      }
    }
  })
  return {
    get now() {
      return now
    },
  }
}
