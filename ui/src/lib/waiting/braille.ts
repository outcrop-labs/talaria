/**
 * The braille renderer: a dot field thresholded into text glyphs.
 *
 * Unicode Braille Patterns (U+2800–U+28FF) encode a 2×4 dot matrix in the low
 * eight bits of the codepoint, which makes a single character an 8-pixel
 * bitmap. A row of five is a 10×4 display that lives INSIDE a line of text: it
 * inherits currentColor, font-size and line-height from its context, so the
 * same indicator works at 11px in a status row and at 24px as a standalone
 * waiting state without a second asset. No SVG spinner does that.
 *
 * The cost of that is one bit per dot. For anything that needs a smooth
 * gradient across the display, see dot-grid.ts.
 */
import { clamp01, type DotField } from './field'

const BRAILLE_BASE = 0x2800

/**
 * Bit index for the dot at (x, y) within one 2×4 cell.
 *
 * The dot numbering is NOT row-major. Dots 1–6 fill the top three rows
 * COLUMN-first (inherited from six-dot braille), and the eight-dot extension
 * bolts the fourth row on afterwards as dots 7 and 8. Assuming row-major here
 * is the classic braille-canvas bug: everything still renders, but the bottom
 * row detaches from the shape and animates on its own.
 */
const bitFor = (x: number, y: number): number => (y < 3 ? x * 3 + y : 6 + x)

/** One rendered character plus its own opacity — the comet trail needs both. */
export interface Cell {
  ch: string
  alpha: number
}

export const cellChar = (mask: number): string => String.fromCharCode(BRAILLE_BASE + mask)

/**
 * Rasterise a field into `cols` braille characters at phase `p`.
 *
 * Allocation-free apart from the returned array: this runs for every visible
 * indicator on every frame the output actually changes, so it stays a tight
 * loop over 8 dots per cell rather than building an intermediate grid.
 */
export function rasterise(
  cols: number,
  p: number,
  field: DotField,
  cellAlpha?: (c: number, p: number, cols: number) => number,
): Cell[] {
  const w = cols * 2
  const out: Cell[] = new Array(cols)
  for (let c = 0; c < cols; c++) {
    let mask = 0
    for (let x = 0; x < 2; x++) {
      for (let y = 0; y < 4; y++) {
        if (field(c * 2 + x, y, p, w, 4) >= 0.5) mask |= 1 << bitFor(x, y)
      }
    }
    out[c] = { ch: cellChar(mask), alpha: cellAlpha ? clamp01(cellAlpha(c, p, cols)) : 1 }
  }
  return out
}

/** Cheap identity for a frame, so the DOM is only touched when it changed. */
export const cellsKey = (cells: Cell[]): string =>
  cells.map((c) => (c.alpha === 1 ? c.ch : `${c.ch}${c.alpha.toFixed(2)}`)).join('')
