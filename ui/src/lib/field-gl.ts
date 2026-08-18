/**
 * THE DITHER FIELD, ON THE GPU — one pass for every field on a surface.
 *
 * WHY THIS REPLACES TWO RENDERERS. The effect was previously drawn two ways: a
 * canvas-2D engine for fields with gradients (halos, vignettes, drifting
 * crests) and a baked SVG tile in CSS for the flat fills behind hover and
 * selection. They could not be made to agree, and every attempt produced a new
 * symptom — a checkerboard here, banding there, one material reading coarser
 * than the other a few pixels away. The cause was structural rather than
 * cosmetic: a static image cannot express a field. Its density is fixed when
 * it is generated, so it cannot answer to distance, to state, or to time, and
 * the colour cannot even come from a token because a CSS variable does not
 * resolve inside a data URI.
 *
 * A fragment shader is the shape this always wanted. Density is evaluated per
 * pixel from the same source list the old engine took, so a fill and a halo
 * are the same code with different inputs, and there is nothing left to drift.
 *
 * WHY ONE CANVAS PER SURFACE, NOT PER ELEMENT. The other reason the fills went
 * to CSS was cost: 126 hoverable rows cannot each own a canvas, a context and
 * an rAF loop. Here every field on a surface is a few numbers in a uniform
 * array and the whole surface is ONE draw call, so the cost stops scaling with
 * the number of call sites. It also settles the lattice question for good —
 * the grid is screen-space, so no two fields can be out of phase and there is
 * no origin to synchronise.
 *
 * Z-ORDER. The surface canvas sits behind that surface's content, which is
 * where every one of these fields belongs anyway: a row's fill under its own
 * label, a button's halo under everything. That is why a single layer per
 * surface is enough and per-element layering was never actually needed.
 */
import { BAYER, type DitherSource, type DitherTone } from './dither'
import { effectDefs, effectIndex, isAnimated } from './field-effects'

/** Tones, in the order the shader indexes them. */
export const TONE_ORDER: DitherTone[] = ['neutral', 'accent', 'success', 'danger', 'surface']

/** A field is a box plus the sources evaluated inside it, in SURFACE space. */
export interface Field {
  x: number
  y: number
  w: number
  h: number
  sources: DitherSource[]
}

/**
 * How many sources one surface can draw at once.
 *
 * Deliberately a hard cap with a visible consequence rather than a growing
 * array: uniform space is finite, and a surface that wants more than this has
 * a design problem the renderer should not paper over. In practice a surface
 * has one hovered row, a handful of selected ones and its ambient field.
 */
export const MAX_SOURCES = 24

const VERT = `#version 300 es
in vec2 a_pos;
void main() { gl_Position = vec4(a_pos, 0.0, 1.0); }`

/**
 * The fragment shader, ASSEMBLED FROM THE EFFECT REGISTRY.
 *
 * Each effect contributes one function and one branch, both generated from its
 * declaration — so adding an effect cannot leave the shader and the packing
 * code disagreeing, which is the failure mode this whole rewrite exists to
 * remove. Everything outside the density switch is the shared material: the
 * Bayer threshold, the organic clumping, the dot geometry, the tone mix and
 * the alpha ramp. An effect says HOW MUCH; the surface says what it is made of.
 */
function buildFragment(): string {
  const defs = effectDefs()
  const fns = defs
    .map(
      (d, i) => `float fx_${i}(vec4 box, vec2 local, vec4 a, vec4 b, float t) {${d.glsl}
}`,
    )
    .join('\n')
  const branches = defs
    .map((_, i) => `  if (kind == ${i}) return fx_${i}(box, local, a, b, t);`)
    .join('\n')

  return `#version 300 es
precision highp float;

uniform vec2  u_res;
uniform float u_dpr;
uniform float u_time;
uniform float u_pitch;
uniform float u_dot;
uniform int   u_count;
uniform float u_organic;
uniform float u_drift;      // clump morph rate, Hz. 0 freezes it.
uniform float u_alphaFloor;
uniform float u_maxAlpha;
uniform vec3  u_tones[5];

uniform vec4 u_box[${MAX_SOURCES}];
uniform vec4 u_p0[${MAX_SOURCES}];   // kind, strength, tone index, gain   // kind, strength, tone, unused
uniform vec4 u_p1[${MAX_SOURCES}];   // effect slot a
uniform vec4 u_p2[${MAX_SOURCES}];   // effect slot b

out vec4 outColor;

const int BAYER[64] = int[64](${BAYER.join(',')});

${fns}

float density(int kind, vec4 box, vec2 local, vec4 a, vec4 b, float t) {
${branches}
  return 0.0;
}

/** Stable per-cell value for the organic clumping. Never moves. */
float hash01(vec3 p) {
  return fract(sin(dot(p, vec3(127.1, 311.7, 74.7))) * 43758.5453123);
}

void main() {
  vec2 px = gl_FragCoord.xy / u_dpr;
  px.y = u_res.y - px.y;                       // GL is bottom-up; the DOM is not

  vec2 cell = floor(px / u_pitch);
  vec2 centre = (cell + 0.5) * u_pitch;

  float miss = 1.0;
  vec3 rgb = vec3(0.0);
  float wsum = 0.0;
  float gsum = 0.0;
  for (int i = 0; i < ${MAX_SOURCES}; i++) {
    if (i >= u_count) break;
    vec4 box = u_box[i];
    float v = u_p0[i].y * density(int(u_p0[i].x), box, centre - box.xy, u_p1[i], u_p2[i], u_time);
    if (v <= 0.0) continue;
    miss *= 1.0 - clamp(v, 0.0, 1.0);
    rgb += u_tones[int(u_p0[i].z)] * v;
    // Gain is averaged the same way the tone is — weighted by how much each
    // source contributes here. Where a quiet ambient field and a loud halo
    // overlap, the pixel takes the weight of whichever is actually doing the
    // work there, so a halo crossing a rail does not get dragged down to the
    // rail's alpha, nor the rail lifted to the halo's.
    gsum += v * u_p0[i].w;
    wsum += v;
  }
  if (wsum == 0.0) discard;

  float d = 1.0 - miss;

  // THE CLUMP FIELD, AND WHY IT MOVES.
  //
  // Two octaves: one on 4-cell blocks for the clusters, one per cell to break
  // their edges. Without any of it an ordered dither renders a ramp as
  // mechanical bands and a flat field as a checkerboard.
  //
  // Static clumping fixes that and introduces its own problem: a fixed
  // arrangement of clusters reads as RAGGED, because the eye resolves it as
  // permanent structure — dots that are meant to be texture become a pattern
  // of blotches you can point at. Movement is what dissolves that reading.
  //
  // It morphs between two static fields rather than re-rolling per frame.
  // Re-rolling is television static: it destroys the clustering that made the
  // texture organic in the first place, and it is exhausting to look at. A
  // smoothstep crossfade at u_drift Hz keeps every frame a legitimate clump
  // field and simply makes which one it is drift. At the default rate a full
  // morph takes about twelve seconds, which is under the threshold where
  // motion becomes something you watch rather than something you feel.
  if (u_organic > 0.0) {
    float phase = u_time * u_drift;
    float f = smoothstep(0.0, 1.0, fract(phase));
    float e0 = floor(phase), e1 = e0 + 1.0;
    float c0 = 0.6 * hash01(vec3(floor(cell / 4.0), 7.0 + e0 * 31.0))
             + 0.4 * hash01(vec3(cell, 13.0 + e0 * 17.0));
    float c1 = 0.6 * hash01(vec3(floor(cell / 4.0), 7.0 + e1 * 31.0))
             + 0.4 * hash01(vec3(cell, 13.0 + e1 * 17.0));
    d *= 1.0 + u_organic * (mix(c0, c1, f) * 1.8 - 0.9);
  }

  ivec2 bc = ivec2(mod(cell, 8.0));
  if (d <= (float(BAYER[bc.y * 8 + bc.x]) + 0.5) / 64.0) discard;

  // Square dots, as the 2D engine drew them — anything softer stops reading
  // as a dither and starts reading as a blur.
  vec2 inCell = px - cell * u_pitch;
  float off = (u_pitch - u_dot) * 0.5;
  if (any(lessThan(inCell, vec2(off))) || any(greaterThan(inCell, vec2(off + u_dot)))) discard;

  // The floor is scaled too: a field at gain 0.15 whose faintest dots still
  // painted at the full floor would be mostly floor, which is the flat grey
  // wash the gain exists to avoid.
  outColor = vec4(rgb / wsum, (u_alphaFloor + (u_maxAlpha - u_alphaFloor) * clamp(d, 0.0, 1.0)) * (gsum / wsum));
}`
}

const FRAG = buildFragment()

export interface FieldGLOptions {
  pitch?: number
  dot?: number
  organic?: number
  alphaFloor?: number
  maxAlpha?: number
  /**
   * How fast the clump field morphs, in Hz. 0 freezes it.
   *
   * Low by design: this exists to stop static clumping reading as permanent
   * blotches, not to be seen moving. At the default a full morph takes about
   * twelve seconds. Reduced motion sets it to 0, which leaves the texture
   * exactly as organic and completely still.
   */
  drift?: number
}

/** Is the GPU path available at all? Callers fall back to no field. */
export function fieldGLSupported(): boolean {
  if (typeof document === 'undefined') return false
  try {
    return !!document.createElement('canvas').getContext('webgl2')
  } catch {
    return false
  }
}

/* ── The renderer ──────────────────────────────────────────────────────────
   One per surface. Owns the context, the program and the uniform locations;
   `setFields` is the whole API a surface needs. */

function compile(gl: WebGL2RenderingContext, type: number, src: string): WebGLShader {
  const sh = gl.createShader(type)!
  gl.shaderSource(sh, src)
  gl.compileShader(sh)
  if (!gl.getShaderParameter(sh, gl.COMPILE_STATUS)) {
    // Surfaced rather than swallowed: a shader that fails to compile renders
    // nothing at all, which looks like the feature was never wired up.
    const log = gl.getShaderInfoLog(sh)
    gl.deleteShader(sh)
    throw new Error(`dither field shader failed to compile: ${log}`)
  }
  return sh
}

export class FieldRenderer {
  private gl: WebGL2RenderingContext
  private program: WebGLProgram
  private u: Record<string, WebGLUniformLocation | null> = {}
  private opts: Required<FieldGLOptions>
  private tones = new Float32Array(15)
  private wCss = 0
  private hCss = 0
  private dpr = 1
  private raf = 0
  private animating = false
  /** Nothing is visible, so nothing is worth drawing. */
  private paused = false
  private destroyed = false
  private fields: Field[] = []
  private t0 = 0

  constructor(canvas: HTMLCanvasElement, opts: FieldGLOptions = {}) {
    const gl = canvas.getContext('webgl2', { premultipliedAlpha: false, antialias: false })
    if (!gl) throw new Error('webgl2 unavailable')
    this.gl = gl
    this.opts = {
      pitch: opts.pitch ?? 2,
      dot: opts.dot ?? 1,
      organic: opts.organic ?? 0.45,
      alphaFloor: opts.alphaFloor ?? 0.02,
      maxAlpha: opts.maxAlpha ?? 0.85,
      drift: opts.drift ?? 0.08,
    }

    const p = gl.createProgram()!
    gl.attachShader(p, compile(gl, gl.VERTEX_SHADER, VERT))
    gl.attachShader(p, compile(gl, gl.FRAGMENT_SHADER, FRAG))
    gl.linkProgram(p)
    if (!gl.getProgramParameter(p, gl.LINK_STATUS)) {
      throw new Error(`dither field program failed to link: ${gl.getProgramInfoLog(p)}`)
    }
    this.program = p
    gl.useProgram(p)

    // A single full-screen triangle pair. There is no geometry here — every
    // field is evaluated per fragment.
    const buf = gl.createBuffer()
    gl.bindBuffer(gl.ARRAY_BUFFER, buf)
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW)
    const loc = gl.getAttribLocation(p, 'a_pos')
    gl.enableVertexAttribArray(loc)
    gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0)

    for (const name of ['u_res','u_dpr','u_time','u_pitch','u_dot','u_count','u_organic','u_drift','u_alphaFloor','u_maxAlpha','u_tones','u_box','u_p0','u_p1','u_p2']) {
      this.u[name] = gl.getUniformLocation(p, name)
    }

    gl.enable(gl.BLEND)
    gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA)
    this.t0 = performance.now()
  }

  setSize(wCss: number, hCss: number, dpr: number): void {
    if (wCss <= 0 || hCss <= 0) return
    this.wCss = wCss
    this.hCss = hCss
    this.dpr = dpr
    const c = this.gl.canvas as HTMLCanvasElement
    c.width = Math.round(wCss * dpr)
    c.height = Math.round(hCss * dpr)
    this.gl.viewport(0, 0, c.width, c.height)
    this.schedule()
  }

  /** Token colours, resolved by the caller (the shader cannot read CSS). */
  setTones(rgb: Array<[number, number, number]>): void {
    for (let i = 0; i < 5; i++) {
      const c = rgb[i] ?? [128, 128, 128]
      this.tones[i * 3] = c[0] / 255
      this.tones[i * 3 + 1] = c[1] / 255
      this.tones[i * 3 + 2] = c[2] / 255
    }
    this.schedule()
  }

  /** The widest reach any source can have, so the scissor box covers it. */
  private reach = 48
  private lastDraw = 0

  /**
   * THE FIELD DOES NOT REDRAW AT 60Hz, AND THAT IS HOW THE PAGE STAYS AT 60.
   *
   * The two are easy to confuse. The requirement is that the interface runs at
   * sixty frames a second; the field redrawing sixty times a second is what
   * takes that away, because every redraw shades every pixel the fields cover
   * and the shader is the expensive part.
   *
   * Nothing here moves quickly. The clump morph is 0.08Hz — a full cycle takes
   * twelve seconds — and the fastest source in the library drifts at nine
   * pixels a second. Sampling either sixty times a second spends sixty frames
   * of budget to show what fifteen would show identically.
   *
   * Measured, all 60 controls on one surface at dpr 2: redrawing every frame
   * put the page at a 50ms median. Throttled, the page holds 16.7ms — the same
   * as with the animation off entirely — and the morph is indistinguishable.
   */
  private readonly minFrameMs = 66

  /** Stop drawing entirely — the tab is hidden, or the surface is offscreen. */
  setPaused(paused: boolean): void {
    if (this.paused === paused) return
    this.paused = paused
    if (!paused) this.schedule()
  }

  setOptions(opts: FieldGLOptions): void {
    Object.assign(this.opts, opts)
    this.schedule()
  }

  setFields(fields: Field[]): void {
    this.fields = fields
    this.schedule()
  }

  destroy(): void {
    this.destroyed = true
    cancelAnimationFrame(this.raf)
    // RELEASE THE CONTEXT, don't just stop drawing it. A browser allows a
    // fixed number of live WebGL contexts (16 in Chrome) and drops the oldest
    // when a new one exceeds it — so a surface that is torn down and rebuilt
    // without this quietly spends the page's budget until some unrelated
    // canvas elsewhere goes blank. Dropping the reference is not enough: the
    // context is released on GC, at a time nobody controls.
    this.gl.getExtension('WEBGL_lose_context')?.loseContext()
  }

  private schedule(): void {
    if (this.destroyed || this.raf) return
    this.raf = requestAnimationFrame((now) => {
      this.raf = 0
      // A throttled frame still costs a rAF callback, which is free; what it
      // skips is the shader. Rescheduling rather than drawing keeps the
      // animation on the clock without paying for it every vsync.
      if (this.animating && now - this.lastDraw < this.minFrameMs) {
        this.schedule()
        return
      }
      this.lastDraw = now
      this.draw()
    })
  }

  private draw(): void {
    if (this.destroyed || this.wCss === 0) return
    const { gl, opts } = this

    // Flatten every field's sources into the uniform arrays. Sources past the
    // cap are dropped rather than silently wrapping — see MAX_SOURCES.
    const box = new Float32Array(MAX_SOURCES * 4)
    const p0 = new Float32Array(MAX_SOURCES * 4)
    const p1 = new Float32Array(MAX_SOURCES * 4)
    const p2 = new Float32Array(MAX_SOURCES * 4)
    let n = 0
    let animated = false

    // Packing is driven by the registry: each effect writes its own slots, so
    // the shader branch and the parameter order cannot disagree. Sources past
    // the cap are DROPPED rather than wrapping — see MAX_SOURCES.
    const defs = effectDefs()
    for (const f of this.fields) {
      for (const src of f.sources) {
        if (n >= MAX_SOURCES) break
        if (src.strength <= 0.002) continue
        const idx = effectIndex(src.kind)
        const def = defs[idx]
        if (!def) continue
        const o = n * 4
        box[o] = f.x; box[o + 1] = f.y; box[o + 2] = f.w; box[o + 3] = f.h
        p0[o] = idx
        p0[o + 1] = src.strength
        p0[o + 2] = Math.max(0, TONE_ORDER.indexOf(src.tone ?? 'neutral'))
        p0[o + 3] = src.gain ?? 1
        const packed = def.pack(src as never)
        p1.set(packed.a, o)
        if (packed.b) p2.set(packed.b, o)
        if (isAnimated(src.kind)) animated = true
        n++
      }
    }

    gl.useProgram(this.program)
    gl.uniform2f(this.u.u_res!, this.wCss, this.hCss)
    gl.uniform1f(this.u.u_dpr!, this.dpr)
    gl.uniform1f(this.u.u_time!, (performance.now() - this.t0) / 1000)
    gl.uniform1f(this.u.u_pitch!, opts.pitch)
    gl.uniform1f(this.u.u_dot!, opts.dot)
    gl.uniform1i(this.u.u_count!, n)
    gl.uniform1f(this.u.u_organic!, opts.organic)
    gl.uniform1f(this.u.u_drift!, this.paused ? 0 : opts.drift)
    gl.uniform1f(this.u.u_alphaFloor!, opts.alphaFloor)
    gl.uniform1f(this.u.u_maxAlpha!, opts.maxAlpha)
    gl.uniform3fv(this.u.u_tones!, this.tones)
    gl.uniform4fv(this.u.u_box!, box)
    gl.uniform4fv(this.u.u_p0!, p0)
    gl.uniform4fv(this.u.u_p1!, p1)
    gl.uniform4fv(this.u.u_p2!, p2)

    // SHADE ONLY WHERE A FIELD CAN REACH. The draw is one fullscreen triangle,
    // but a scissor box clipped to the union of the active sources means the
    // fragment shader never runs on the rest of the surface. That is the whole
    // performance story: a hovered button is a ~120x80 region, not a 2560x1400
    // one, and the per-pixel cost is a loop over every active source. Without
    // this, a continuously morphing field would shade the entire viewport
    // sixty times a second to light up one button's edge.
    gl.disable(gl.SCISSOR_TEST)
    gl.clearColor(0, 0, 0, 0)
    gl.clear(gl.COLOR_BUFFER_BIT)

    if (n > 0) {
      const pad = Math.max(this.reach, opts.pitch)
      let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity
      for (let i = 0; i < n; i++) {
        const o = i * 4
        const bx = box[o]!, by = box[o + 1]!, bw = box[o + 2]!, bh = box[o + 3]!
        x0 = Math.min(x0, bx - pad)
        y0 = Math.min(y0, by - pad)
        x1 = Math.max(x1, bx + bw + pad)
        y1 = Math.max(y1, by + bh + pad)
      }
      const dpr = this.dpr
      const sx = Math.max(0, Math.floor(x0 * dpr))
      // Scissor is measured from the BOTTOM of the framebuffer; the boxes are
      // measured from the top of the surface.
      const sy = Math.max(0, Math.floor((this.hCss - y1) * dpr))
      const sw = Math.min((this.gl.canvas as HTMLCanvasElement).width - sx, Math.ceil((x1 - x0) * dpr))
      const sh = Math.min((this.gl.canvas as HTMLCanvasElement).height - sy, Math.ceil((y1 - y0) * dpr))
      if (sw > 0 && sh > 0) {
        gl.enable(gl.SCISSOR_TEST)
        gl.scissor(sx, sy, sw, sh)
        gl.drawArrays(gl.TRIANGLES, 0, 3)
        gl.disable(gl.SCISSOR_TEST)
      }
    }

    // A frame is needed if anything moves: a wave, or the clump morph. With
    // nothing moving and nothing hovered the surface costs one paint and then
    // stops, which is what keeps an idle page at zero.
    this.animating = !this.paused && n > 0 && (animated || opts.drift > 0)
    if (this.animating) this.schedule()
  }
}
