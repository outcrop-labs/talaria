/**
 * STREAMED TEXT ARRIVES UNRESOLVED AND SETTLES.
 *
 * A token lands, and for a moment it is not yet itself: it shows as code that
 * is not yet the right code — a run of letters and digits that decodes, left
 * to right, into what the model actually said. It is the dither idea applied
 * to language — a character whose value is still statistical, the way a
 * dot's brightness is.
 *
 * PURE ON PURPOSE. Everything here is `(text, arrivals, now) -> string`, with
 * no timers, no DOM and no randomness that is not derived from its inputs. The
 * component around it owns the clock. That is what lets the whole behaviour be
 * asserted in a node test rather than eyeballed against a live model, which
 * matters more here than usual: this only ever runs while an agent is talking,
 * so the interesting states are the ones that are hardest to reproduce by hand.
 */

/**
 * The glyphs an unresolved character can wear.
 *
 * Uppercase and digits — the vocabulary of CODE. A lowercase stand-in starts
 * looking like words the model never said; uppercase-and-digits reads as a
 * matrix decode from the first frame, and it cannot pose as the settled text
 * around it (which is lowercase words). It also removes every markdown-active
 * character from the pool, which the escaper below still guards for the
 * settled syntax the stand-ins sit beside.
 */
const GLYPHS = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789'

/** How long one character spends unresolved, in ms. */
export const SETTLE_MS = 220

/**
 * How often a still-unresolved character picks a new glyph, in ms.
 *
 * Not every frame. A character re-rolling at 60Hz is a strobe — it reads as
 * flicker rather than as indecision, and it makes the line impossible to skim
 * while it lands. At ~13Hz the eye reads each glyph as a distinct wrong guess.
 */
export const CHURN_MS = 75

export interface RevealChar {
  /** What to draw right now — the real character, or a stand-in. */
  ch: string
  /** 0 while unresolved, 1 once settled. Drives the fade. */
  settled: number
}

/**
 * The visible state of `text`, given when each character arrived.
 *
 * `arrivals[i]` is the timestamp character `i` was appended. Characters with no
 * recorded arrival are treated as long settled, which is what makes this safe
 * to call on a message that was already complete when it mounted — history
 * does not animate.
 */
export function reveal(text: string, arrivals: number[], now: number): RevealChar[] {
  const out: RevealChar[] = []
  for (let i = 0; i < text.length; i++) {
    const ch = text[i]!
    const at = arrivals[i]
    // WHITESPACE NEVER SCRAMBLES. Substituting a glyph for a space collapses
    // the word boundaries, so the line re-flows on every churn and the text
    // jitters sideways while it resolves. Newlines are worse: the block would
    // change height. They arrive settled.
    if (at === undefined || ch === ' ' || ch === '\n' || ch === '\t') {
      out.push({ ch, settled: 1 })
      continue
    }
    const age = now - at
    if (age >= SETTLE_MS) {
      out.push({ ch, settled: 1 })
      continue
    }
    const tick = Math.floor(age / CHURN_MS)
    out.push({ ch: standIn(i, tick), settled: age / SETTLE_MS })
  }
  return out
}

/**
 * The stand-in glyph character `i` wears at churn tick `tick`.
 *
 * Exported because the markdown-safe renderer below has to know which glyph is
 * on screen to decide whether it needs escaping, and the tests assert against
 * the exact frame rather than a property of it.
 *
 * RAIN, NOT A STROBE. Each column re-rolls on its own period — even
 * positions every 3 ticks, odd every 4 — with a phase that differs per
 * position, so no whole run of the tail re-rolls on one tick: one or two
 * columns flip at a time and the rest hold. That stagger is what "code
 * generating" looks like; a shared period re-rolls the whole run in one
 * synchronized flash. (A shared period with per-position offset is NOT enough
 * — any offset that is a function of position mod the period keeps whole
 * residue classes in sync; the phase here is `i >> 1 mod period`, which
 * walks the class.)
 */
export function standIn(i: number, tick: number): string {
  const period = 3 + (i & 1)
  const frame = Math.floor((tick + ((i >> 1) % period)) / period)
  return GLYPHS[(i * 31 + frame * 7) % GLYPHS.length]!
}

/**
 * The stand-in glyphs that double as markdown syntax.
 *
 * The scrambled tail is still PARSED: a stand-in `*` beside a settled `*`
 * opens emphasis, a stand-in `>` at a line start opens a blockquote, a
 * stand-in `<` can autolink the settled text that follows it. The real
 * character only appears once the character settles, so the only syntax the
 * stream can introduce early is the stand-in's — and escaping the stand-in
 * closes that door without touching the settled text.
 */
const MD_ACTIVE = new Set(['*', '<', '>', '\\', '|', '~'])

/**
 * Does the text leave a fenced code block open?
 *
 * Inside a fence a backslash is a character, not an escape, so a stand-in
 * must NOT be escaped there — the escape would print. GFM indents a fence up
 * to three spaces, and tilde fences count too.
 */
export function fenceOpen(text: string): boolean {
  const fences = text.match(/^[ ]{0,3}(?:`{3,}|~{3,})/gm)
  return (fences?.length ?? 0) % 2 === 1
}

/**
 * `reveal()` as one markdown-safe string: settled characters verbatim,
 * unsettled characters as stand-in glyphs, escaped where a glyph would
 * otherwise be parsed as syntax — and left raw inside an open code fence,
 * where a backslash would print.
 *
 * Returns the input unchanged when nothing is unsettled, so a settled message
 * costs a comparison, not a rebuild.
 */
export function revealedText(text: string, arrivals: number[], now: number): string {
  const chars = reveal(text, arrivals, now)
  const fence = fenceOpen(text)
  let out = ''
  let dirty = false
  for (const c of chars) {
    if (c.settled === 1) {
      out += c.ch
      continue
    }
    dirty = true
    out += !fence && MD_ACTIVE.has(c.ch) ? '\\' + c.ch : c.ch
  }
  return dirty ? out : text
}

/**
 * Record arrival times for characters appended since the last call.
 *
 * Returns a NEW array; the caller keeps it as the running record. Only growth
 * is treated as arrival — a message that is rewritten wholesale (an edit, a
 * retry, a resume replaying history) has no new characters to resolve, and
 * animating one would be a lie about what just happened.
 */
export function trackArrivals(prev: string, next: string, arrivals: number[], now: number): number[] {
  if (!next.startsWith(prev)) return []
  if (next.length === arrivals.length) return arrivals
  const out = arrivals.slice(0, next.length)
  for (let i = out.length; i < next.length; i++) out[i] = now
  return out
}

/** Is anything still unresolved? Lets the caller stop its clock. */
export function settling(text: string, arrivals: number[], now: number): boolean {
  for (let i = Math.max(0, text.length - 64); i < text.length; i++) {
    const at = arrivals[i]
    if (at !== undefined && now - at < SETTLE_MS) return true
  }
  return false
}

/**
 * How many trailing characters are still unresolved.
 *
 * Arrivals are monotonic (newer characters arrive later), so the unresolved
 * set is always a SUFFIX of the text, holes where whitespace sits. The suffix
 * is what matters: that is the region a renderer has to re-draw — and the
 * region where stand-in glyphs live in the rendered DOM.
 */
export function unsettledCount(text: string, arrivals: number[], now: number): number {
  let n = 0
  for (let i = text.length - 1; i >= 0; i--) {
    const ch = text[i]!
    // Whitespace never scrambles, so it is never in the region — but it does
    // not end it: an unresolved character can sit just past a space.
    if (ch === ' ' || ch === '\n' || ch === '\t') continue
    const at = arrivals[i]
    if (at === undefined || now - at >= SETTLE_MS) break
    n++
  }
  return n
}
