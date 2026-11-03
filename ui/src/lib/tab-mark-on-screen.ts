import { SvelteSet } from 'svelte/reactivity'
import type { WaitingSiteKey } from './waiting/sites'

// The tab marks a viewer can see right now, by site. The readout repeats the
// lead tab's mark. While that tab's own mark is on screen the copy holds still:
// a fourth mark in motion cost the main thread more and showed nothing new.
// At a narrow width the strip scrolls the lead tab away, and then the copy is
// the only mark of that view in sight, so it moves again.
const onScreen = new SvelteSet<WaitingSiteKey>()

export const tabMarkOnScreen = (site: WaitingSiteKey): boolean => onScreen.has(site)

/** Attachment for a tab's mark: records whether any of it is on screen. */
export function watchTabMark(site: WaitingSiteKey) {
  return (node: Element) => {
    const io = new IntersectionObserver(([entry]) => {
      if (entry?.isIntersecting) onScreen.add(site)
      else onScreen.delete(site)
    })
    io.observe(node)
    return () => {
      io.disconnect()
      onScreen.delete(site)
    }
  }
}
