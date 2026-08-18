/**
 * WHO IS DRAWING A FIELD ON THIS SURFACE.
 *
 * A surface (the app shell, a modal) owns one WebGL canvas; every control that
 * wants a field registers here instead of mounting a canvas of its own. That
 * inversion is the point of the shader rewrite: 126 hoverable rows become 126
 * entries in an array and one draw call, rather than 126 contexts.
 *
 * Registration is by ELEMENT, not by coordinates. The registry re-reads each
 * element's box at draw time, so a field follows its control through scrolling,
 * a collapsing rail or a reflow without anyone having to remember to
 * re-measure. An entry whose element has left the document is dropped on the
 * next pass rather than leaking.
 */
import { getContext, setContext } from 'svelte'
import type { DitherSource, DitherTone } from './dither'

// `Symbol.for`, not `Symbol()`. A bare symbol is unique per MODULE INSTANCE,
// and dev tooling routinely produces more than one of those: Vite serves a
// hot-updated module under a new URL, so a component compiled before the
// update and one compiled after hold different symbols — `setContext` and
// `getContext` then silently disagree and every field on the page vanishes
// with no error to follow. The global symbol registry keys on the string, so
// all instances agree. Production has one instance and does not care; this is
// purely so a dev-only failure cannot masquerade as a broken feature.
const KEY = Symbol.for('talaria.dither-field-surface')

export interface FieldEntry {
  /** The control the field belongs to. Its box is read fresh each draw. */
  el: HTMLElement
  /** Evaluated per draw, so a caller can vary the field with its own state. */
  sources: () => DitherSource[]
}

export interface FieldSurfaceApi {
  register(entry: FieldEntry): () => void
  /** Ask for a redraw — state changed in a way the surface cannot observe. */
  invalidate(): void
}

/** The surface component calls this; controls below it call `useFieldSurface`. */
export function provideFieldSurface(api: FieldSurfaceApi): void {
  setContext(KEY, api)
}

/**
 * The surface this control sits on, or null when there is none.
 *
 * Null is a legitimate answer and callers must handle it: a component can be
 * rendered outside any surface (a test, a detached preview), and the honest
 * response is to draw no field rather than to throw or to mount a private
 * canvas and reintroduce the cost this design removed.
 */
export function useFieldSurface(): FieldSurfaceApi | null {
  return getContext<FieldSurfaceApi | undefined>(KEY) ?? null
}

/**
 * Draw a field for this control, on whatever surface it happens to sit on.
 *
 * Call during component init (it reads context and owns effects). Handing it a
 * GETTER for the element rather than the element itself matters: the ref is
 * null on the first run and the control may be re-created by a keyed block, so
 * a value captured once would register a node that is no longer on the page.
 *
 * Returning no sources is the way to turn the field OFF — an un-hovered
 * button contributes nothing and costs nothing, which is what lets every
 * control on a surface register unconditionally.
 */
export function useField(el: () => HTMLElement | null, sources: () => DitherSource[]): void {
  const surface = useFieldSurface()
  if (!surface) return

  $effect(() => {
    const e = el()
    if (!e) return
    return surface.register({ el: e, sources })
  })

  // The surface cannot observe a caller's state, so a change in what the
  // sources SAY has to be pushed. Compared by value: callers rebuild the array
  // on every state change and only a real change should cost a redraw.
  const key = $derived(JSON.stringify(sources()))
  $effect(() => {
    void key
    surface.invalidate()
  })
}

/**
 * A DITHERED FILL FOR ANY ELEMENT, drawn by the surface.
 *
 * This replaces the CSS `dither-fill` utility, and the reason is not purity.
 * That utility was a background image under a mask on a pseudo-element, and
 * every one of those layers is work the BROWSER does on its own thread: a
 * 64px tile resampled per element, a mask composited per element, all of it
 * recomputed on scroll and on theme change. It pins the render cycle to the
 * main thread precisely where the UI is busiest — a list of rows under a
 * moving pointer. The same fill as a field costs one more entry in a uniform
 * array and no compositing at all.
 *
 * It is a factory rather than an action because context is only readable
 * during component init: call `createDitherFill()` in the script block, then
 * attach the result to as many elements as you like.
 *
 *   const fill = createDitherFill()
 *   <button {@attach fill}>…
 *
 * `active` makes the fill permanent for a selected row; otherwise it appears
 * on hover and on keyboard focus, so the two reach the control the same way.
 */
export function createDitherFill(opts: {
  /** Solid-state companion: keep the fill up regardless of pointer. */
  active?: () => boolean
  tone?: DitherTone
  /** Peak density. The fill is flat, so this is the whole of its weight. */
  strength?: number
} = {}) {
  const surface = useFieldSurface()

  return (node: HTMLElement) => {
    if (!surface) return
    let hot = $state(false)
    const on = () => (hot = true)
    const off = () => (hot = false)
    node.addEventListener('mouseenter', on)
    node.addEventListener('mouseleave', off)
    node.addEventListener('focusin', on)
    node.addEventListener('focusout', off)

    const stop = surface.register({
      el: node,
      sources: () =>
        hot || opts.active?.()
          ? [{ id: 'fill', kind: 'uniform', strength: opts.strength ?? 0.5, tone: opts.tone ?? 'neutral' }]
          : [],
    })

    return () => {
      node.removeEventListener('mouseenter', on)
      node.removeEventListener('mouseleave', off)
      node.removeEventListener('focusin', on)
      node.removeEventListener('focusout', off)
      stop()
    }
  }
}
