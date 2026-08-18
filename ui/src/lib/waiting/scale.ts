/**
 * A dot grid needs more box than a glyph needs em to read at the same weight.
 * A braille glyph fills most of its em; a 5×5 lattice spends 38% of its box on
 * gaps, so matched px sizes make the grid look markedly smaller than the text
 * beside it.
 *
 * Lives in its own module because both the indicator dispatcher and the page
 * that describes the two families need it, and importing a constant out of a
 * `.svelte` component to get it would be the wrong dependency direction.
 */
export const GRID_SCALE = 1.7
