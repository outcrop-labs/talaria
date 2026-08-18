// Motion grammar for the Svelte side, replacing framer-motion's
// <MotionConfig reducedMotion="user">: every transition in the app goes
// through these wrappers, which collapse to a fade-only (entrances lose
// travel/scale) when the OS asks for reduced motion — spec §9. CSS motifs are
// handled by the prefers-reduced-motion block in styles.css.
//
// Usage is identical to svelte/transition:
//   <div transition:fade>            import { fade } from '@/lib/motion'
//   <div in:fly={{ y: 8 }}>          import { fly } from '@/lib/motion'
import { flip as svFlip, type FlipParams, type AnimationConfig } from 'svelte/animate'
import { quintOut } from 'svelte/easing'
import {
  crossfade as svCrossfade,
  fade as svFade,
  fly as svFly,
  scale as svScale,
  slide as svSlide,
  type FadeParams,
  type FlyParams,
  type ScaleParams,
  type SlideParams,
  type TransitionConfig,
} from 'svelte/transition'

const REDUCED_MOTION = '(prefers-reduced-motion: reduce)'

/** The query every wrapper below degrades on. Exported for code that has to
 *  ASK rather than be wrapped — the canvas fields animate outside Svelte's
 *  transition system, so they cannot go through `fade`/`fly`. */
export function prefersReducedMotion(): boolean {
  return typeof window !== 'undefined' && window.matchMedia(REDUCED_MOTION).matches
}

/**
 * Subscribe to reduced-motion CHANGES, with one listener for the whole page.
 *
 * `cb` fires immediately with the current value, so a subscriber never has to
 * read the preference and register a listener as two separate steps (which is
 * how the two canvas fields each ended up with their own matchMedia).
 *
 * Ref-counted: the listener exists only while something is subscribed, which
 * for a canvas field means only while one is mounted.
 */
export function onReducedMotion(cb: (reduced: boolean) => void): () => void {
  cb(prefersReducedMotion())
  if (typeof window === 'undefined') return () => {}

  motionSubs.add(cb)
  if (!motionMq) {
    motionMq = window.matchMedia(REDUCED_MOTION)
    motionMq.addEventListener('change', fanOutMotion)
  }
  return () => {
    motionSubs.delete(cb)
    if (motionSubs.size === 0 && motionMq) {
      motionMq.removeEventListener('change', fanOutMotion)
      motionMq = null
    }
  }
}

/* ── The page clock ────────────────────────────────────────────────────────
 * ONE requestAnimationFrame loop for every looping indicator on the page.
 *
 * Lives here rather than in the subsystem that first needed it: the waiting
 * marks and the skeleton static both drive canvas/DOM animation outside
 * Svelte's transition system, and a general primitive's engine should not
 * import from a feature folder to get its clock. This is the same reasoning
 * that put `onReducedMotion` above it.
 *
 * A busy view can have a chat turn, several tool rows, a background monitor and
 * a screen of skeletons running at once; independent rAF loops would each wake
 * the compositor on their own schedule and turn a cheap update into a
 * measurable cost. Subscribers that want a slower cadence (the skeleton field
 * buckets to 8Hz) do it on top of this rather than beside it.
 *
 * Every subscriber shares a single origin, so things that start at different
 * moments still beat in phase. Two indicators visibly drifting against each
 * other reads as two unrelated things breaking; in phase it reads as one
 * system working.
 *
 * rAF is already suspended in a background tab, so there is no visibility
 * guard here — one would be belt-and-braces over a guarantee the platform
 * makes.
 */
type ClockSubscriber = (elapsedMs: number) => void

const clockSubs = new Set<ClockSubscriber>()
let clockHandle = 0
let clockOrigin = 0

function clockFrame(now: number): void {
  if (clockOrigin === 0) clockOrigin = now
  const elapsed = now - clockOrigin
  for (const fn of clockSubs) fn(elapsed)
  clockHandle = requestAnimationFrame(clockFrame)
}

/** Subscribe to the shared frame loop. Returns an unsubscribe. */
export function subscribeToClock(fn: ClockSubscriber): () => void {
  clockSubs.add(fn)
  if (clockHandle === 0) clockHandle = requestAnimationFrame(clockFrame)
  return () => {
    clockSubs.delete(fn)
    if (clockSubs.size === 0) {
      cancelAnimationFrame(clockHandle)
      clockHandle = 0
      clockOrigin = 0
    }
  }
}

const motionSubs = new Set<(reduced: boolean) => void>()
let motionMq: MediaQueryList | null = null
const fanOutMotion = (): void => {
  const reduced = prefersReducedMotion()
  for (const cb of motionSubs) cb(reduced)
}

/** Reduced motion degrades to a quick fade rather than nothing: an element
 *  that pops in with zero signal reads as a glitch, not as calm. */
function degrade(node: Element, params?: { duration?: number; delay?: number }): TransitionConfig {
  return svFade(node, { duration: Math.min(params?.duration ?? 150, 150), delay: params?.delay })
}

export function fade(node: Element, params?: FadeParams): TransitionConfig {
  return svFade(node, params)
}

export function fly(node: Element, params?: FlyParams): TransitionConfig {
  return prefersReducedMotion() ? degrade(node, params) : svFly(node, params)
}

export function scale(node: Element, params?: ScaleParams): TransitionConfig {
  return prefersReducedMotion() ? degrade(node, params) : svScale(node, params)
}

export function slide(node: Element, params?: SlideParams): TransitionConfig {
  return prefersReducedMotion() ? degrade(node, params) : svSlide(node, params)
}

/** Keyed-list reorder animation (`animate:flip`), reduced-motion-aware like
 *  the transitions above. Under reduced motion items jump — position change
 *  is information, and a fade can't carry it, so instant is the honest form. */
export function flip(node: HTMLElement, positions: { from: DOMRect; to: DOMRect }, params?: FlipParams): AnimationConfig {
  return prefersReducedMotion() ? { duration: 0 } : svFlip(node, positions, params)
}

// ── Round 2: perceptible, elegant ────────────────────────────────────────────
// The 0.96-scale/160ms first pass read as "completely unanimated". Motion that
// can't be felt is cost without signal: entrances now rise AND scale on a
// quint curve — still calm, unmistakably alive.

const EASE = 'cubic-bezier(0.22, 1, 0.36, 1)' // quintOut, as a CSS string for WAAPI

/** Overlay entrance: rise + scale + fade on one quint curve. The one
 *  transition for modals and popovers (scale alone was imperceptible). */
export function pop(
  node: Element,
  params?: { duration?: number; delay?: number; y?: number; start?: number },
): TransitionConfig {
  if (prefersReducedMotion()) return degrade(node, params)
  const { duration = 300, delay, y = 12, start = 0.94 } = params ?? {}
  return {
    duration,
    delay,
    easing: quintOut,
    css: (t, u) => `opacity: ${t}; transform: translateY(${u * y}px) scale(${start + t * (1 - start)})`,
  }
}

/** Animated resize. Put this on a wrapper whose ONLY child is the measured
 *  content (AutoHeight.svelte gives you the structure); when the content's
 *  height changes — a wizard step swap, a disclosure, an error row — the
 *  wrapper's height glides instead of snapping. WAAPI, native, no layout
 *  thrash: the content is already at its final size, only the clip animates. */
export function autoHeight(node: HTMLElement, opts?: { duration?: number }) {
  const inner = node.firstElementChild
  if (!inner) return
  let last = node.offsetHeight
  let anim: Animation | null = null
  const ro = new ResizeObserver(() => {
    const next = (inner as HTMLElement).offsetHeight
    if (next === last || last === 0) {
      last = next
      return
    }
    if (!prefersReducedMotion()) {
      anim?.cancel()
      const prevOverflow = node.style.overflow
      node.style.overflow = 'hidden'
      anim = node.animate([{ height: `${last}px` }, { height: `${next}px` }], {
        duration: opts?.duration ?? 220,
        easing: EASE,
      })
      anim.onfinish = anim.oncancel = () => {
        node.style.overflow = prevOverflow
      }
    }
    last = next
  })
  ro.observe(inner)
  return {
    destroy() {
      ro.disconnect()
      anim?.cancel()
    },
  }
}

/** Staggered content entrance: direct children rise in, 50ms apart. Put it on
 *  the container INSIDE a `{#key …}` block (or on a view/loaded-content
 *  container) so mounts re-run it. `fill: backwards` hides delayed children
 *  until their turn; `data-no-stagger` sits an element out.
 *
 *  NESTED CASCADE: a direct child marked `data-stagger-items` is treated as a
 *  section whose ITEMS cascade within the section's slot — the section frame
 *  fades in place (no travel: frame + items both translating reads as double
 *  motion) while its items rise 30ms apart starting at the frame's delay.
 *  By default the items are the marked element's direct children; give the
 *  attribute a selector value (`data-stagger-items="li"`) when the list sits
 *  deeper (e.g. inside a Panel body). Items beyond `maxItems` appear together
 *  with the last cadence slot — a 200-row table must not take 6 seconds.
 *
 *  Reduced motion: everything simply appears. */
export function staggerIn(
  node: HTMLElement,
  opts?: { step?: number; duration?: number; y?: number; maxSteps?: number; itemStep?: number; maxItems?: number },
) {
  if (prefersReducedMotion()) return
  const { step = 50, duration = 320, y = 10, maxSteps = 8, itemStep = 30, maxItems = 12 } = opts ?? {}
  const rise = (el: Element, delay: number) =>
    el.animate(
      [
        { opacity: 0, transform: `translateY(${y}px)` },
        { opacity: 1, transform: 'translateY(0)' },
      ],
      { duration, delay, easing: EASE, fill: 'backwards' },
    )
  Array.from(node.children).forEach((el, i) => {
    if (el.hasAttribute('data-no-stagger')) return
    const base = Math.min(i, maxSteps) * step
    const itemsSel = el.getAttribute('data-stagger-items')
    if (itemsSel === null) {
      rise(el, base)
      return
    }
    // Section frame: fade only, at its slot. Items: rise within the slot.
    el.animate([{ opacity: 0 }, { opacity: 1 }], { duration, delay: base, easing: EASE, fill: 'backwards' })
    const items = itemsSel === '' ? Array.from(el.children) : Array.from(el.querySelectorAll(`:scope ${itemsSel}`))
    items.forEach((item, j) => {
      if (item.hasAttribute('data-no-stagger')) return
      if (j >= maxItems) return // beyond the cap: present immediately (see listStagger)
      rise(item, base + (j + 1) * itemStep)
    })
  })
}

/** THE RULE OF THUMB: any grid or list staggers its items subtly on mount.
 *  Put this on the element whose direct children are the items ({#each}
 *  container). Quieter than staggerIn — less travel, tighter cadence — and
 *  capped: items beyond `maxItems` appear together. If the list sits inside a
 *  `use:staggerIn` container, also mark it `data-no-stagger` so the section
 *  cascade skips it and this cascade owns it. */
export function listStagger(
  node: HTMLElement,
  opts?: { step?: number; duration?: number; y?: number; maxItems?: number },
) {
  if (prefersReducedMotion()) return
  const { step = 30, duration = 280, y = 6, maxItems = 12 } = opts ?? {}
  Array.from(node.children).forEach((el, i) => {
    if (el.hasAttribute('data-no-stagger')) return
    // Items beyond the cap don't animate AT ALL — they're simply there.
    // (Clamping only the delay made a 200-row list sit invisible and then
    // materialize as a block: the whole list read as one slow stagger. The
    // cascade is for the first screenful; below the fold, presence wins.)
    if (i >= maxItems) return
    el.animate(
      [
        { opacity: 0, transform: `translateY(${y}px)` },
        { opacity: 1, transform: 'translateY(0)' },
      ],
      { duration, delay: i * step, easing: EASE, fill: 'backwards' },
    )
  })
}

/** A send/receive pair for the one element that MOVES between spots — a tab
 *  underline, a segmented thumb. Make one pair per component:
 *    const [sendMark, receiveMark] = markCrossfade()
 *    {#if active === tab.id}<div in:receiveMark={{key:'mark'}} out:sendMark={{key:'mark'}} …>
 *  Reduced motion falls back to a quick fade (the fallback leg). */
export function markCrossfade(duration = 200) {
  return svCrossfade({
    duration: () => (prefersReducedMotion() ? 0 : duration),
    easing: quintOut,
    fallback: (node) => svFade(node, { duration: 100 }),
  })
}

// House defaults, so surfaces feel like one system rather than 141 opinions:
//   modals            in:pop            out:fade={QUICK}
//   popovers/menus    in:pop={POPOVER}  out:fade={QUICK}   (origin-* toward trigger)
//   panels/drawers    in:fly={PANEL} (or x: PANEL_X)  out:fade={QUICK}
//   step/tab content  {#key} + use:staggerIn, wrapped in AutoHeight when the
//                     container height changes with the content
//   list/row reveal   slide (height) or fade, ≤150ms
//   reorder/move      animate:flip={LIST} on the keyed {#each} items
//   banners           slide
//   view changes      NOTHING — deliberately. A cross-view transition paints a
//                     static snapshot while the incoming view is still loading,
//                     so the live DOM lands in one jump when it ends. The
//                     content's own entrance (Materialize / staggerIn) is the
//                     animation; see the note in styles.css.
// Round-3 tempo: round 2 was mechanically correct and still read as "stupid
// fast". These are the slowest values that stay calm — entrances you can
// actually watch, exits that don't make anyone wait.
export const QUICK: FadeParams = { duration: 160 }
export const POP = { duration: 300, y: 12, start: 0.94 }
export const POPOVER = { duration: 220, y: 8, start: 0.95 }
export const PANEL: FlyParams = { duration: 320, y: 18, easing: quintOut }
export const PANEL_X: FlyParams = { duration: 320, x: 20, easing: quintOut }
// IN-FLOW panels (they occupy layout, siblings must glide): grow open, shrink
// closed — `transition:slide={GROW_X}` on both legs. fly/PANEL_X is ONLY for
// OVERLAY panels (portaled, out of flow) where nothing reflows around them;
// on an in-flow panel fly animates the transform while the width lands
// instantly, so the layout snap eats the animation.
export const GROW_X: SlideParams = { axis: 'x', duration: 300, easing: quintOut }
export const GROW_Y: SlideParams = { axis: 'y', duration: 300, easing: quintOut }
export const LIST: FlipParams = { duration: 250 }
