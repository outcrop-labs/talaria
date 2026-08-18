/**
 * THE EFFECT LIBRARY — every dither effect the UI can draw, declared once.
 *
 * An effect is a DENSITY FUNCTION over a control's box. That is the whole
 * abstraction: given a point, how much of the field is here. Everything else —
 * the Bayer threshold, the organic clumping, the dot geometry, the tone mix,
 * the alpha ramp — is the shared material and belongs to the renderer, not to
 * the effect.
 *
 * ADDING ONE IS A SINGLE DECLARATION. Before this, a new kind meant editing a
 * TS union, an index map, a GLSL branch and a packing routine — four places
 * that had to agree, which is precisely how the earlier CSS and canvas paths
 * drifted apart. Here the type, the packing and the shader source sit in one
 * object and the renderer assembles the shader from them, so a mismatch is a
 * compile error rather than a rendering difference nobody notices.
 *
 * WHAT AN EFFECT MAY NOT DO, and this is the load-bearing constraint: it
 * cannot choose its own grain. Pitch, dot size and organic clumping are
 * properties of the SURFACE, shared by everything drawn on it. The single most
 * expensive mistake in this feature's history was having two renderers with
 * two grains a few pixels apart, and per-effect grain would rebuild that
 * problem inside one renderer. An effect says how much; the surface says what
 * it is made of.
 *
 * The GLSL each effect contributes is the body of a function receiving:
 *   vec4  box    the control's box in surface space (x, y, w, h)
 *   vec2  local  the sample point relative to that box
 *   vec4  a, b   the effect's own packed parameters
 *   float t      seconds, for anything that moves
 * and returning density in 0..1, before strength is applied.
 */
import type { DitherTone } from './dither'

/** Every effect writes its parameters into two vec4 slots. */
export type Slot = [number, number, number, number]

export interface EffectDef<P> {
  /** Stable name, used in source objects and for the GLSL function name. */
  kind: string
  /** Density body. `return` a float in 0..1. */
  glsl: string
  /** Pack the effect's own params. Anything not set defaults to 0. */
  pack(params: P): { a: Slot; b?: Slot }
}

/** Fields common to every source, owned by the renderer rather than an effect. */
export interface SourceBase {
  /** Stable identity, so a caller can rebuild its list without restarting. */
  id: string
  /** Peak contribution, 0..1. Setting it to 0 is how an effect turns off. */
  strength: number
  tone?: DitherTone
}

const defs = new Map<string, EffectDef<never>>()
const order: string[] = []

/** Register an effect. Call at module load; the renderer reads the set once. */
export function defineEffect<P>(def: EffectDef<P>): EffectDef<P> {
  if (defs.has(def.kind)) throw new Error(`duplicate dither effect: ${def.kind}`)
  defs.set(def.kind, def as EffectDef<never>)
  order.push(def.kind)
  return def
}

export const effectIndex = (kind: string): number => order.indexOf(kind)
export const effectDefs = (): ReadonlyArray<EffectDef<never>> => order.map((k) => defs.get(k)!)

/* ── The built-in library ───────────────────────────────────────────────────
   Each of these was a branch in the original engine. They are the vocabulary
   the UI draws with today; a new one is added the same way. */

/** A halo around the control's box — the button bloom, the tab pool. */
export const rectEffect = defineEffect<{
  spread: number
  radius?: number
  /** Level deep inside the box. 0 keeps the interior clean for a label. */
  inner?: number
  /** Depth of the boundary-hugging band inside the box. */
  rim?: number
  /** Decay exponent outside. Higher starts the blend-out at the edge. */
  falloff?: number
}>({
  kind: 'rect',
  glsl: `
    vec2 hb = box.zw * 0.5;
    float r = min(a.y, min(hb.x, hb.y));
    vec2 q = abs(local - hb) - hb + r;
    float sd = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
    if (sd < 0.0) {
      if (a.z >= 1.0) return 1.0;
      if (a.w <= 0.0) return a.z;
      float k = clamp(1.0 + sd / a.w, 0.0, 1.0);
      return a.z + (1.0 - a.z) * k * k;
    }
    if (sd >= a.x) return 0.0;
    return pow(1.0 - sd / a.x, max(b.x, 0.001));`,
  pack: (p) => ({ a: [p.spread, p.radius ?? 0, p.inner ?? 1, p.rim ?? 10], b: [p.falloff ?? 2, 0, 0, 0] }),
})

/** Density at one edge of the box, falling off inward — vignettes, corridors. */
export const edgeEffect = defineEffect<{ side: 'top' | 'bottom' | 'left' | 'right'; depth: number }>({
  kind: 'edge',
  glsl: `
    float d = a.x < 0.5 ? local.y
            : a.x < 1.5 ? box.w - local.y
            : a.x < 2.5 ? local.x
                        : box.z - local.x;
    return pow(clamp(1.0 - d / a.y, 0.0, 1.0), 2.1);`,
  pack: (p) => ({ a: [{ top: 0, bottom: 1, left: 2, right: 3 }[p.side], p.depth, 0, 0] }),
})

/** Flat density everywhere — grain, or a dissolve veil. */
export const uniformEffect = defineEffect<Record<string, never>>({
  kind: 'uniform',
  glsl: `return 1.0;`,
  pack: () => ({ a: [0, 0, 0, 0] }),
})

/** Radial falloff from a point inside the box. */
export const haloEffect = defineEffect<{ x: number; y: number; radius: number }>({
  kind: 'halo',
  glsl: `
    float d = distance(local, a.xy);
    if (d >= a.z) return 0.0;
    float f = 1.0 - d / a.z;
    return f * f;`,
  pack: (p) => ({ a: [p.x, p.y, p.radius, 0] }),
})

/** Linear ramp along an axis — a progress fill with a dissolving edge. */
export const rampEffect = defineEffect<{
  axis: 'x' | 'y'
  from: number
  to: number
  fromLevel: number
  toLevel: number
}>({
  kind: 'ramp',
  glsl: `
    float c = a.x < 0.5 ? local.x : local.y;
    float t01 = a.z == a.y ? 1.0 : clamp((c - a.y) / (a.z - a.y), 0.0, 1.0);
    return b.x + (b.y - b.x) * t01;`,
  pack: (p) => ({ a: [p.axis === 'x' ? 0 : 1, p.from, p.to, 0], b: [p.fromLevel, p.toLevel, 0, 0] }),
})

/**
 * A travelling crest — the only effect that moves on its own.
 *
 * Cubed so crests read as bands rather than a smooth sine, and never negative:
 * a signed crest would SUBTRACT from the other sources sharing the pixel
 * rather than contributing nothing.
 */
export const waveEffect = defineEffect<{ axis: 'x' | 'y'; wavelength: number; speed: number }>({
  kind: 'wave',
  glsl: `
    float c = a.x < 0.5 ? local.x : local.y;
    float crest = 0.5 + 0.5 * sin(((c - a.z * t) / a.y) * 6.2831853);
    return crest * crest * crest;`,
  pack: (p) => ({ a: [p.axis === 'x' ? 0 : 1, p.wavelength, p.speed, 0] }),
})

/** Does this effect need a frame after the current one? */
export const isAnimated = (kind: string): boolean => kind === 'wave'
