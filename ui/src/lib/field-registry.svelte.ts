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
import type { DitherSource } from './dither'

const KEY = Symbol('dither-field-surface')

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
