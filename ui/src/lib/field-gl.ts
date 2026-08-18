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

const KIND_INDEX: Record<DitherSource['kind'], number> = {
  rect: 0,
  edge: 1,
  halo: 2,
  ramp: 3,
  uniform: 4,
  wave: 5,
}

const VERT = `#version 300 es
in vec2 a_pos;
void main() { gl_Position = vec4(a_pos, 0.0, 1.0); }`

// The fragment shader IS the engine. Every branch below has a counterpart in
// `evalSource` in lib/dither.ts, and the two must keep saying the same thing —
// the unit tests on that function are the specification for this.
const FRAG = `#version 300 es
precision highp float;

uniform vec2  u_res;        // surface size in CSS px
uniform float u_dpr;
uniform float u_time;       // seconds, for wave sources
uniform float u_pitch;      // grid pitch in CSS px
uniform float u_dot;        // dot size in CSS px
uniform int   u_count;
uniform float u_organic;
uniform float u_alphaFloor;
uniform float u_maxAlpha;
uniform vec3  u_tones[5];

uniform vec4 u_box[${MAX_SOURCES}];   // x, y, w, h        (surface space)
uniform vec4 u_p0[${MAX_SOURCES}];    // kind, strength, tone, falloff
uniform vec4 u_p1[${MAX_SOURCES}];    // kind-specific, see below
uniform vec4 u_p2[${MAX_SOURCES}];    // kind-specific

out vec4 outColor;

const int BAYER[64] = int[64](${BAYER.join(',')});

float clamp01(float v) { return clamp(v, 0.0, 1.0); }

// The engine's hash01, in float. Not bit-identical to the JS integer version —
// GLSL has no cheap 32-bit integer mixing — but it serves the same purpose:
// a stable per-cell value that never moves. The clumps differ in placement
// from the old canvas engine's, not in character.
float hash01(vec3 p) {
  return fract(sin(dot(p, vec3(127.1, 311.7, 74.7))) * 43758.5453123);
}

/** Signed distance to a rounded box: negative inside. */
float sdRoundBox(vec2 p, vec2 halfSize, float r) {
  r = min(r, min(halfSize.x, halfSize.y));
  vec2 q = abs(p) - halfSize + r;
  return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
}

float density(int i, vec2 px) {
  vec4 box = u_box[i];
  vec4 p0 = u_p0[i], p1 = u_p1[i], p2 = u_p2[i];
  int kind = int(p0.x);
  float strength = p0.y, falloff = p0.w;
  vec2 local = px - box.xy;          // position within the field's own box

  if (kind == 4) return strength;    // uniform

  if (kind == 1) {                   // edge: side, depth
    int side = int(p1.x);
    float depth = p1.y;
    float d = side == 0 ? local.y
            : side == 1 ? box.w - local.y
            : side == 2 ? local.x
                        : box.z - local.x;
    return strength * pow(clamp01(1.0 - d / depth), 2.1);
  }

  if (kind == 0) {                   // rect: spread, radius, inner, rim
    float spread = p1.x, radius = p1.y, inner = p1.z, rim = p1.w;
    vec2 halfBox = box.zw * 0.5;
    float sd = sdRoundBox(local - halfBox, halfBox, radius);
    if (sd < 0.0) {
      if (inner >= 1.0) return strength;
      if (rim <= 0.0) return strength * inner;
      float k = clamp01(1.0 + sd / rim);
      return strength * (inner + (1.0 - inner) * k * k);
    }
    if (sd >= spread) return 0.0;
    return strength * pow(1.0 - sd / spread, falloff);
  }

  if (kind == 2) {                   // halo: cx, cy, radius
    float d = distance(local, p1.xy);
    if (d >= p1.z) return 0.0;
    float f = 1.0 - d / p1.z;
    return strength * f * f;
  }

  if (kind == 3) {                   // ramp: axis, from, to, fromLevel/toLevel
    float c = p1.x < 0.5 ? local.x : local.y;
    float from = p1.y, to = p1.z;
    float t = to == from ? 1.0 : clamp01((c - from) / (to - from));
    return strength * (p2.x + (p2.y - p2.x) * t);
  }

  // wave: axis, wavelength, speed
  float c = p1.x < 0.5 ? local.x : local.y;
  float crest = 0.5 + 0.5 * sin(((c - p1.z * u_time) / p1.y) * 6.2831853);
  return strength * crest * crest * crest;
}

void main() {
  vec2 px = gl_FragCoord.xy / u_dpr;
  px.y = u_res.y - px.y;                       // GL is bottom-up; the DOM is not

  // Snap to the grid cell centre, so a dot is a cell and not a smear.
  vec2 cell = floor(px / u_pitch);
  vec2 centre = (cell + 0.5) * u_pitch;

  float miss = 1.0;
  vec3 rgb = vec3(0.0);
  float wsum = 0.0;
  for (int i = 0; i < ${MAX_SOURCES}; i++) {
    if (i >= u_count) break;
    float v = density(i, centre);
    if (v <= 0.0) continue;
    miss *= 1.0 - clamp01(v);
    vec3 tone = u_tones[int(u_p0[i].z)];
    rgb += tone * v;
    wsum += v;
  }
  if (wsum == 0.0) discard;

  float d = 1.0 - miss;

  // The organic term, as in the engine: two octaves of static clump noise,
  // one on 4-cell blocks for the clusters and one per cell to break their
  // edges. Without it an ordered dither renders a ramp as mechanical bands
  // and a flat field as a checkerboard.
  if (u_organic > 0.0) {
    float clump = 0.6 * hash01(vec3(floor(cell / 4.0), 7.0))
                + 0.4 * hash01(vec3(cell, 13.0));
    d *= 1.0 + u_organic * (clump * 1.8 - 0.9);
  }

  ivec2 b = ivec2(mod(cell, 8.0));
  float threshold = (float(BAYER[b.y * 8 + b.x]) + 0.5) / 64.0;
  if (d <= threshold) discard;

  // Dots are drawn as squares of u_dot within the cell, like the engine's
  // fillRect — anything softer stops reading as a dither.
  vec2 inCell = px - cell * u_pitch;
  float off = (u_pitch - u_dot) * 0.5;
  if (any(lessThan(inCell, vec2(off))) || any(greaterThan(inCell, vec2(off + u_dot)))) discard;

  float alpha = u_alphaFloor + (u_maxAlpha - u_alphaFloor) * clamp01(d);
  outColor = vec4(rgb / wsum, alpha);
}`

export interface FieldGLOptions {
  pitch?: number
  dot?: number
  organic?: number
  alphaFloor?: number
  maxAlpha?: number
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

    for (const name of ['u_res','u_dpr','u_time','u_pitch','u_dot','u_count','u_organic','u_alphaFloor','u_maxAlpha','u_tones','u_box','u_p0','u_p1','u_p2']) {
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
  }

  private schedule(): void {
    if (this.destroyed || this.raf) return
    this.raf = requestAnimationFrame(() => {
      this.raf = 0
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
    let hasWave = false

    for (const f of this.fields) {
      for (const s of f.sources) {
        if (n >= MAX_SOURCES) break
        if (s.strength <= 0.002) continue
        const b = n * 4
        box[b] = f.x; box[b + 1] = f.y; box[b + 2] = f.w; box[b + 3] = f.h
        p0[b] = KIND_INDEX[s.kind]
        p0[b + 1] = s.strength
        p0[b + 2] = TONE_ORDER.indexOf(s.tone ?? 'neutral')
        p0[b + 3] = s.kind === 'rect' ? (s.falloff ?? 2) : 2
        if (s.kind === 'rect') {
          p1[b] = s.spread; p1[b + 1] = s.radius ?? 0
          p1[b + 2] = s.inner ?? 1; p1[b + 3] = s.rim ?? 10
        } else if (s.kind === 'edge') {
          p1[b] = { top: 0, bottom: 1, left: 2, right: 3 }[s.side]
          p1[b + 1] = s.depth
        } else if (s.kind === 'halo') {
          p1[b] = s.x - f.x; p1[b + 1] = s.y - f.y; p1[b + 2] = s.radius
        } else if (s.kind === 'ramp') {
          p1[b] = s.axis === 'x' ? 0 : 1; p1[b + 1] = s.from; p1[b + 2] = s.to
          p2[b] = s.fromLevel; p2[b + 1] = s.toLevel
        } else if (s.kind === 'wave') {
          p1[b] = s.axis === 'x' ? 0 : 1; p1[b + 1] = s.wavelength; p1[b + 2] = s.speed
          hasWave = true
        }
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
    gl.uniform1f(this.u.u_alphaFloor!, opts.alphaFloor)
    gl.uniform1f(this.u.u_maxAlpha!, opts.maxAlpha)
    gl.uniform3fv(this.u.u_tones!, this.tones)
    gl.uniform4fv(this.u.u_box!, box)
    gl.uniform4fv(this.u.u_p0!, p0)
    gl.uniform4fv(this.u.u_p1!, p1)
    gl.uniform4fv(this.u.u_p2!, p2)

    gl.clearColor(0, 0, 0, 0)
    gl.clear(gl.COLOR_BUFFER_BIT)
    if (n > 0) gl.drawArrays(gl.TRIANGLES, 0, 3)

    // Only a wave needs the next frame. Everything else is static once drawn,
    // so an idle surface costs nothing — the same rule the 2D engine followed.
    this.animating = hasWave
    if (this.animating) this.schedule()
  }
}
