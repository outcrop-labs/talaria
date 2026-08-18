/**
 * `prefers-reduced-motion`, as a REACTIVE value.
 *
 * `@/lib/motion` owns the subscription: `onReducedMotion` is ONE matchMedia
 * listener for the whole page, shared with the canvas fields (the loading
 * skeletons, the buttons' bloom). What this module adds is the shape the two
 * waiting renderers need — a `$state` that re-renders them when the preference
 * CHANGES, rather than a boolean sampled once on the way past, which is what a
 * transition wants and what `prefersReducedMotion()` is for.
 *
 * Started by the first indicator to render and never torn down: it is a single
 * listener for one boolean, and in a busy cockpit something is nearly always
 * waiting on something.
 */
import { onReducedMotion } from '@/lib/motion'

let reduced = $state(false)
let started = false

function start(): void {
  if (started) return
  started = true
  // Fires immediately with the current preference, so the first read is right.
  onReducedMotion((next) => (reduced = next))
}

/** Read as `.current` — a bare boolean returned at init would freeze. */
export function usePrefersReducedMotion(): { readonly current: boolean } {
  start()
  return {
    get current() {
      return reduced
    },
  }
}
