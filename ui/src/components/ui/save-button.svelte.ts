// The one "Save → Saved ✓" pattern: useSavedFlash gives the timed flag so
// settings panels stop re-rolling setTimeout state machines. (It once had a
// SaveButton wrapper; every real site wanted its own button, so the hook is
// the whole primitive.)
import { onDestroy } from 'svelte'

/** Call during component init. Read `saved` off the returned object (don't
 *  destructure it — the getter is what keeps it reactive). */
export function useSavedFlash(ms = 1500): { readonly saved: boolean; flash: () => void } {
  let saved = $state(false)
  let t: ReturnType<typeof setTimeout> | null = null
  onDestroy(() => {
    if (t) clearTimeout(t)
  })
  return {
    get saved() {
      return saved
    },
    flash: () => {
      saved = true
      if (t) clearTimeout(t)
      t = setTimeout(() => (saved = false), ms)
    },
  }
}
